//! End-to-end handler tests on SQLite, with a mock PLC directory.

mod audiences;
mod auth_features;
mod blobs;
mod bulk;
mod conformance;
mod console_features;
mod identity;
mod keys;
mod keys_shard;
mod protocol;
mod security;

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
        "service": op["services"]
            .as_object()
            .map(|m| m.iter().map(|(id, svc)| json!({
                "id": format!("#{id}"),
                "type": svc["type"],
                "serviceEndpoint": svc["endpoint"],
            })).collect::<Vec<_>>())
            .unwrap_or_default(),
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

/// The blob store tests use: disk under the test's directory, or, with
/// `LOCUS_TEST_S3_ENDPOINT` set, the S3-compatible service there
/// (`LOCUS_TEST_S3_BUCKET`, default `locus-test`, and
/// `LOCUS_TEST_S3_GATED_BUCKET` if gated blobs get a bucket of their own;
/// `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` sign in). Every key is
/// unique, so tests can share a bucket.
fn test_blob_store(dir: &std::path::Path) -> locus_blob::BlobStore {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let Some(endpoint) = var("LOCUS_TEST_S3_ENDPOINT") else {
        return locus_blob::BlobStore::disk(locus_blob::DiskStore::new(dir.join("blobs")));
    };
    let config = locus_blob::S3Config {
        bucket: var("LOCUS_TEST_S3_BUCKET").unwrap_or_else(|| "locus-test".into()),
        gated_bucket: var("LOCUS_TEST_S3_GATED_BUCKET"),
        region: None,
        endpoint: Some(endpoint),
        force_path_style: true,
        access_key_id: var("AWS_ACCESS_KEY_ID").unwrap_or_default(),
        secret_access_key: var("AWS_SECRET_ACCESS_KEY").unwrap_or_default(),
        session_token: None,
        upload_timeout: std::time::Duration::from_secs(30),
        request_timeout: None,
    };
    locus_blob::BlobStore::s3(locus_blob::S3Store::new(&config).unwrap())
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
    harness_build(HarnessOpts {
        hostname: hostname.into(),
        invite_required,
        email,
        ..HarnessOpts::default()
    })
    .await
}

/// A PDS in setup mode, its bootstrap hostname `hostname`.
async fn setup_harness(hostname: &str, email: bool) -> Harness {
    harness_build(HarnessOpts {
        hostname: hostname.into(),
        email,
        setup_mode: true,
        ..HarnessOpts::default()
    })
    .await
}

/// A shard at pds.example.com behind `entryway`.
async fn shard_harness(entryway: crate::shard::Entryway) -> Harness {
    harness_build(HarnessOpts {
        entryway: Some(Arc::new(entryway)),
        ..HarnessOpts::default()
    })
    .await
}

/// How a test node is built.
struct HarnessOpts {
    hostname: String,
    invite_required: bool,
    email: bool,
    setup_mode: bool,
    /// The shard role, behind this entryway.
    entryway: Option<Arc<crate::shard::Entryway>>,
    /// The entryway role, with this token key.
    entryway_key: Option<proto_blue_crypto::K256Keypair>,
    /// A PLC directory shared with another node.
    plc: Option<(String, Plc)>,
    /// Instead of `https://<hostname>`.
    public_url: Option<String>,
    /// Rate limits and replay windows in this Redis.
    kv: crate::scratch::Scratch,
    /// Off: user-influenced fetches may reach loopback (mock remote PDSes).
    ssrf_protection: bool,
    /// An explicit service DID (`PDS_SERVICE_DID`), recorded as at setup.
    service_did: Option<String>,
    /// The env the server secrets are first loaded from (else the process's).
    env: Option<HashMap<String, String>>,
}

impl Default for HarnessOpts {
    fn default() -> Self {
        Self {
            hostname: "pds.example.com".into(),
            invite_required: false,
            email: true,
            setup_mode: false,
            entryway: None,
            entryway_key: None,
            plc: None,
            public_url: None,
            kv: crate::scratch::Scratch::default(),
            ssrf_protection: true,
            service_did: None,
            env: None,
        }
    }
}

/// The harness's database: SQLite in `dir`, or a fresh Postgres database
/// when `LOCUS_TEST_HTTP_POSTGRES_URL` names a server (check.sh runs the
/// suite both ways).
async fn test_database(dir: &std::path::Path) -> DatabaseUrl {
    let Ok(admin_url) = std::env::var("LOCUS_TEST_HTTP_POSTGRES_URL") else {
        return DatabaseUrl::Sqlite(dir.join("t.sqlite"));
    };
    let admin = sqlx::PgPool::connect(&admin_url).await.unwrap();
    let db = format!("locus_test_{}", hex::encode(rand::random::<[u8; 8]>()));
    sqlx::query(&format!("CREATE DATABASE {db}"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    let (base, _) = admin_url
        .rsplit_once('/')
        .expect("LOCUS_TEST_HTTP_POSTGRES_URL ends in /<database>");
    DatabaseUrl::Postgres(format!("{base}/{db}"))
}

/// The SQLite pool, for tests that set up or inspect rows directly; None
/// on Postgres, where such a test skips (the store tests cover those
/// paths on both backends).
fn sqlite_pool(store: &Store) -> Option<&sqlx::SqlitePool> {
    match store {
        Store::Sqlite(p) => Some(p),
        Store::Postgres(_) => None,
    }
}

async fn harness_build(o: HarnessOpts) -> Harness {
    let HarnessOpts {
        hostname,
        invite_required,
        email,
        setup_mode,
        entryway,
        entryway_key,
        plc,
        public_url,
        kv,
        ssrf_protection,
        service_did,
        env,
    } = o;
    let hostname = hostname.as_str();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&test_database(dir.path()).await).await.unwrap();
    let (plc_url, plc) = match plc {
        Some(p) => p,
        None => mock_plc().await,
    };
    store
        .set_setting("identity.plc_url", json!(plc_url), "test")
        .await
        .unwrap();
    store
        .set_setting("accounts.invite_required", json!(invite_required), "test")
        .await
        .unwrap();
    let master_key = Arc::new(MasterKey::generate(SourceKind::Env));
    let secrets = match &env {
        Some(env) => ServerSecrets::load_or_create_from(&store, &master_key, env).await,
        None => ServerSecrets::load_or_create(&store, &master_key).await,
    }
    .unwrap();
    crate::identity::apply_service_did_env(&store, service_did.as_deref(), false)
        .await
        .unwrap();
    let settings = Settings::load(store.clone()).await.unwrap();
    let hostname = hostname.to_owned();
    let hostname_for_gate = hostname.clone();
    let service_did = crate::identity::service_did_for(&settings.current(), &hostname);
    let mut sessions = LegacySessions::new(store.clone(), secrets.jwt.clone(), service_did.clone());
    if let Some(k) = &entryway_key {
        let private = proto_blue_crypto::ExportableKeypair::export_private_key(k);
        sessions = sessions.with_entryway(
            crate::entryway::session_keys(
                proto_blue_crypto::K256Keypair::from_private_key(&private).unwrap(),
            )
            .unwrap(),
        );
        let mut tx = store.begin().await.unwrap();
        tx.put_secret(crate::entryway::TOKEN_KEY_SECRET, &private, &master_key)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let (mailer, mail) = if email {
        locus_mail::Mailer::capturing(store.clone(), Arc::clone(&master_key))
    } else {
        (
            locus_mail::Mailer::new(store.clone(), Arc::clone(&master_key)),
            Arc::default(),
        )
    };
    let firehose = locus_sync::Firehose::new(store.clone(), 500);
    let (kr, audit_rx) = crate::kryphocron::Kr::new(secrets.correlation_key);
    tokio::spawn(crate::kryphocron::run_audit_writer(
        store.clone(),
        audit_rx,
        tokio_util::sync::CancellationToken::new(),
    ));
    let gate_settings = Arc::clone(&settings);
    let blob_epoch = store.blob_backend().await.unwrap().epoch;
    let state = Arc::new(AppState {
        repo: RepoEngine::new(store.clone(), Arc::new(TidClock::new(0)))
            .with_media_gate(move || crate::media_gate::gate_on_in(&gate_settings.current())),
        store,
        settings,
        master_key,
        secrets,
        sessions,
        mailer,
        blobs: test_blob_store(dir.path()).with_epoch(blob_epoch),
        scratch: locus_blob::DiskStore::new(dir.path()),
        firehose,
        public_url: public_url.unwrap_or_else(|| format!("https://{hostname}")),
        hostname,
        service_did,
        node_id: "test-node".into(),
        ssrf_protection,
        resolvers: crate::state::ResolverCache::default(),
        limits: crate::ratelimit::Limiter::default(),
        kv,
        oauth_clients: crate::oauth::ClientCache::default(),
        kr,
        setup: Arc::new(if setup_mode {
            crate::setup::Gate::setup_mode(
                Some(hostname_for_gate.clone()),
                false,
                crate::setup::SetupInfo {
                    database: "SQLite".into(),
                    blob_store: "disk".into(),
                    data_dir: dir.path().to_path_buf(),
                    ..crate::setup::SetupInfo::default()
                },
            )
        } else {
            crate::setup::Gate::finished()
        }),
        logs: Arc::new(locus_core::logs::LogRing::new(100)),
        branding: crate::branding::Cache::default(),
        socket_secret: crate::socket::new_secret(),
        reload: tokio_util::sync::CancellationToken::new(),
        hosts: arc_swap::ArcSwap::from_pointee(crate::hostnames::Hosts::default()),
        role: if entryway_key.is_some() {
            locus_core::bootstrap::Role::Entryway
        } else if entryway.is_some() {
            locus_core::bootstrap::Role::Shard
        } else {
            locus_core::bootstrap::Role::Pds
        },
        dev_mode: false,
        entryway,
    });
    crate::kryphocron::refresh_oracle(&state).await.unwrap();
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
    let rotation = h.state.secrets.plc_keys.current_did().unwrap();
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
        // A taken handle is InvalidRequest here, as the reference
        // (HandleNotAvailable on updateHandle).
        (
            base("erin.pds.example.com", "other@example.com"),
            "InvalidRequest",
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
    assert!(signed.verify(&h.state.secrets.plc_keys.current_did().unwrap()));

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
        rotation_keys: vec![h.state.secrets.plc_keys.current_did().unwrap()],
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
    let genesis = h
        .state
        .secrets
        .plc_keys
        .current()
        .unwrap()
        .sign_op(&op)
        .await
        .unwrap();
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
            .verify(&h.state.secrets.plc_keys.current_did().unwrap())
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

    /// A procedure call; `{}` to a method with no input sends no body
    /// (such methods refuse one).
    async fn post(&self, path: &str, auth: Option<&str>, body: Value) -> (StatusCode, Value) {
        let body = (!(NO_INPUT.contains(&path) && body == json!({}))).then_some(body);
        self.call("POST", &format!("/xrpc/{path}"), auth, body)
            .await
    }
}

/// The procedures whose lexicon has no input.
const NO_INPUT: &[&str] = &[
    "com.atproto.identity.requestPlcOperationSignature",
    "com.atproto.server.activateAccount",
    "com.atproto.server.deleteSession",
    "com.atproto.server.refreshSession",
    "com.atproto.server.requestAccountDelete",
    "com.atproto.server.requestEmailConfirmation",
    "com.atproto.server.requestEmailUpdate",
];

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
    let current = h.state.secrets.plc_keys.current_did().unwrap();
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
    let moved = h
        .state
        .secrets
        .plc_keys
        .current()
        .unwrap()
        .sign_op(op)
        .await
        .unwrap();
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
    // One code a millisecond: let more than five intervals pass.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
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
    // Grace 0: step past the millisecond it was orphaned in.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let mut tx = h.state.store.begin().await.unwrap();
    assert_eq!(tx.gc_blob_batch(3_600_000, 0, 100, 1).await.unwrap(), 1);
    tx.commit().await.unwrap();
    let (st, _, _) = h.raw("GET", &path, None, Vec::new(), &[]).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "gone");
    // Its bytes go after the delete grace (0 here; step past the
    // millisecond it was queued in).
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    assert!(
        locus_blob::run_deletes(&h.state.store, &h.state.blobs, 0, 100)
            .await
            .unwrap()
            >= 1
    );
    assert!(
        h.state
            .blobs
            .read(&row.storage_key, None)
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

#[tokio::test]
async fn bodies_and_records_have_live_limits() {
    let h = harness(false).await;
    let acct = h
        .create_account("eli.pds.example.com", "eli@example.com", "pw")
        .await;
    let (did, access) = (
        acct["did"].as_str().unwrap().to_owned(),
        acct["accessJwt"].as_str().unwrap().to_owned(),
    );
    // Other endpoints take 150 KB; record writes 2 MB.
    let big = json!({"identifier": "x".repeat(200_000), "password": "pw"});
    let (st, v) = h.post("com.atproto.server.createSession", None, big).await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::PAYLOAD_TOO_LARGE, Some("PayloadTooLarge"))
    );
    let record = json!({"repo": did, "collection": "com.example.big", "record": {"blob": "x".repeat(200_000)}});
    let (st, v) = h
        .post(
            "com.atproto.repo.createRecord",
            Some(&access),
            record.clone(),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    // Lowered live.
    h.state
        .settings
        .set("limits.record_size", json!(100_000), "test")
        .await
        .unwrap();
    let (st, v) = h
        .post("com.atproto.repo.createRecord", Some(&access), record)
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(
        v["message"]
            .as_str()
            .unwrap()
            .starts_with("Record too large"),
        "{v}"
    );
    h.state
        .settings
        .set("limits.repo_write_body", json!(50_000), "test")
        .await
        .unwrap();
    let small = json!({"repo": did, "collection": "com.example.big", "record": {"blob": "x".repeat(60_000)}});
    let (st, _) = h
        .post("com.atproto.repo.createRecord", Some(&access), small)
        .await;
    assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE);
}

/// Reads the next binary firehose frame, with a timeout.
async fn next_frame<S>(ws: &mut S) -> proto_blue_ws::frame::Frame
where
    S: futures::Stream<
            Item = Result<
                tokio_tungstenite::tungstenite::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + Unpin,
{
    use futures::StreamExt;
    loop {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("a frame within 5 s")
            .expect("stream open")
            .expect("no error");
        if let tokio_tungstenite::tungstenite::Message::Binary(b) = msg {
            return proto_blue_ws::frame::Frame::decode(&b).unwrap();
        }
    }
}

fn message(frame: proto_blue_ws::frame::Frame) -> (String, i64) {
    match frame {
        proto_blue_ws::frame::Frame::Message(m) => (
            m.r#type.unwrap_or_default(),
            m.body.as_map().unwrap()["seq"].as_integer().unwrap(),
        ),
        proto_blue_ws::frame::Frame::Error(e) => panic!("error frame {}", e.error),
    }
}

#[tokio::test]
async fn the_firehose_backfills_then_streams_live() {
    let h = harness(false).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, h.app.clone()).into_future());
    let cancel = tokio_util::sync::CancellationToken::new();
    tokio::spawn(
        Arc::clone(&h.state.firehose)
            .run_tail(cancel.clone(), std::time::Duration::from_millis(20)),
    );
    let lease = h
        .state
        .store
        .try_acquire_lease("sequencer", "test", std::time::Duration::from_secs(60))
        .await
        .unwrap()
        .unwrap();
    let sequence = || async {
        let n = locus_sync::sequence_once(&h.state.store, &lease, 500, 0)
            .await
            .unwrap();
        h.state.firehose.wake();
        n
    };

    let acct = h
        .create_account("fay.pds.example.com", "fay@example.com", "pw")
        .await;
    let (did, access) = (
        acct["did"].as_str().unwrap().to_owned(),
        acct["accessJwt"].as_str().unwrap().to_owned(),
    );
    assert_eq!(sequence().await, 4);
    assert!(h.outbox_kinds(&did).await.is_empty(), "the outbox drained");

    // Backfill from the start, in order, with gapless seqs.
    let url = |q: &str| format!("ws://{addr}/xrpc/com.atproto.sync.subscribeRepos{q}");
    let (mut ws, _) = tokio_tungstenite::connect_async(url("?cursor=0"))
        .await
        .unwrap();
    let mut got = Vec::new();
    for _ in 0..4 {
        got.push(message(next_frame(&mut ws).await));
    }
    assert_eq!(
        got,
        vec![
            ("#identity".into(), 1),
            ("#account".into(), 2),
            ("#commit".into(), 3),
            ("#sync".into(), 4)
        ]
    );

    // Then live: a new commit arrives on the same stream, and on a
    // live-only subscriber connected before it.
    let (mut live_only, _) = tokio_tungstenite::connect_async(url("")).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    h.post(
        "com.atproto.repo.createRecord",
        Some(&access),
        json!({"repo": did, "collection": "app.bsky.feed.post", "record": post_record("live")}),
    )
    .await;
    assert_eq!(sequence().await, 1);
    assert_eq!(message(next_frame(&mut ws).await), ("#commit".into(), 5));
    assert_eq!(
        message(next_frame(&mut live_only).await),
        ("#commit".into(), 5)
    );

    // A cursor ahead of the log is refused.
    let (mut future, _) = tokio_tungstenite::connect_async(url("?cursor=999"))
        .await
        .unwrap();
    match next_frame(&mut future).await {
        proto_blue_ws::frame::Frame::Error(e) => assert_eq!(e.error, "FutureCursor"),
        other => panic!("expected FutureCursor, got {other:?}"),
    }

    // Behind retention: OutdatedCursor, then the oldest retained event.
    assert_eq!(h.state.store.prune_seq_events(-1).await.unwrap(), 5);
    h.post(
        "com.atproto.repo.createRecord",
        Some(&access),
        json!({"repo": did, "collection": "app.bsky.feed.post", "record": post_record("after prune")}),
    )
    .await;
    sequence().await;
    let (mut old, _) = tokio_tungstenite::connect_async(url("?cursor=1"))
        .await
        .unwrap();
    match next_frame(&mut old).await {
        proto_blue_ws::frame::Frame::Message(m) => {
            assert_eq!(m.r#type.as_deref(), Some("#info"));
            assert_eq!(
                m.body.as_map().unwrap()["name"].as_str(),
                Some("OutdatedCursor")
            );
        }
        other => panic!("expected #info, got {other:?}"),
    }
    assert_eq!(message(next_frame(&mut old).await), ("#commit".into(), 6));
    cancel.cancel();
}

#[tokio::test]
async fn sync_reads_serve_verifiable_cars_and_status() {
    use proto_blue_repo::proofs::verify_key_in_proof;
    let h = harness(false).await;
    let acct = h
        .create_account("gus.pds.example.com", "gus@example.com", "pw")
        .await;
    let (did, access) = (
        acct["did"].as_str().unwrap().to_owned(),
        acct["accessJwt"].as_str().unwrap().to_owned(),
    );
    let mut cids = Vec::new();
    for (rkey, text) in [("3m4aaaaaaaaaa", "one"), ("3m4aaaaaaaaab", "two")] {
        let (_, v) = h
            .post(
                "com.atproto.repo.createRecord",
                Some(&access),
                json!({"repo": did, "collection": "app.bsky.feed.post", "rkey": rkey, "record": post_record(text)}),
            )
            .await;
        cids.push(v["cid"].as_str().unwrap().to_owned());
    }
    let signing = h.state.signing_key(&did).await.unwrap().did();

    // The whole repo verifies against the account's key.
    let (st, headers, car) = h
        .raw(
            "GET",
            &format!("/xrpc/com.atproto.sync.getRepo?did={did}"),
            None,
            vec![],
            &[],
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(headers["content-type"], "application/vnd.ipld.car");
    let repo = proto_blue_repo::sync::verify_repo_car(&car, Some(&did), Some(&signing)).unwrap();
    let (_, latest) = h
        .get(format!("/xrpc/com.atproto.sync.getLatestCommit?did={did}"))
        .await;
    assert_eq!(
        repo.commit_cid.to_string_base32(),
        latest["cid"].as_str().unwrap()
    );

    // A record proof, and a proof of absence.
    let proof = |rkey: &str| {
        h.raw_get(format!(
            "/xrpc/com.atproto.sync.getRecord?did={did}&collection=app.bsky.feed.post&rkey={rkey}"
        ))
    };
    let (_, _, car) = proof("3m4aaaaaaaaab").await;
    let (root, blocks) = proto_blue_repo::car::read_car_with_root(&car).unwrap();
    let commit = proto_blue_repo::commit::SignedCommit::from_lex_value(
        &proto_blue_lex_cbor::decode(blocks.get(&root).unwrap()).unwrap(),
    )
    .unwrap();
    let want = proto_blue_lex_data::Cid::from_str_multibase(&cids[1]).unwrap();
    assert!(
        verify_key_in_proof(
            &blocks,
            &commit.data,
            "app.bsky.feed.post/3m4aaaaaaaaab",
            Some(&want)
        )
        .unwrap()
    );
    assert!(blocks.get(&want).is_some(), "the record block is included");
    let (_, _, car) = proof("3m4zzzzzzzzzz").await;
    let (root, blocks) = proto_blue_repo::car::read_car_with_root(&car).unwrap();
    let commit = proto_blue_repo::commit::SignedCommit::from_lex_value(
        &proto_blue_lex_cbor::decode(blocks.get(&root).unwrap()).unwrap(),
    )
    .unwrap();
    assert!(
        verify_key_in_proof(
            &blocks,
            &commit.data,
            "app.bsky.feed.post/3m4zzzzzzzzzz",
            None
        )
        .unwrap(),
        "absent"
    );

    // getBlocks: named blocks, or a refusal naming the missing ones.
    let (st, _, car) = h
        .raw(
            "GET",
            &format!(
                "/xrpc/com.atproto.sync.getBlocks?did={did}&cids={}&cids={}",
                cids[0], cids[1]
            ),
            None,
            vec![],
            &[],
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        proto_blue_repo::car::read_car(&car)
            .unwrap()
            .1
            .iter()
            .count(),
        2
    );
    let absent = proto_blue_lex_data::Cid::for_cbor(b"nope").to_string_base32();
    let (st, _, _) = h
        .raw(
            "GET",
            &format!("/xrpc/com.atproto.sync.getBlocks?did={did}&cids={absent}"),
            None,
            vec![],
            &[],
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // Status, listRepos.
    let (_, status) = h
        .get(format!("/xrpc/com.atproto.sync.getRepoStatus?did={did}"))
        .await;
    assert_eq!(
        (status["active"].as_bool(), status["rev"].as_str()),
        (Some(true), latest["rev"].as_str())
    );
    let (_, list) = h.get("/xrpc/com.atproto.sync.listRepos".into()).await;
    assert_eq!(list["repos"][0]["did"], did.as_str());
    assert_eq!(list["repos"][0]["head"], latest["cid"]);

    // Deactivated: refused to everyone but the owner; status still answers.
    h.post(
        "com.atproto.server.deactivateAccount",
        Some(&access),
        json!({}),
    )
    .await;
    let (st, v) = h
        .get(format!("/xrpc/com.atproto.sync.getLatestCommit?did={did}"))
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("RepoDeactivated"))
    );
    let (st, _) = h
        .call(
            "GET",
            &format!("/xrpc/com.atproto.sync.getLatestCommit?did={did}"),
            Some(&access),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "the owner bypasses");
    let (_, status) = h
        .get(format!("/xrpc/com.atproto.sync.getRepoStatus?did={did}"))
        .await;
    assert_eq!(
        (status["active"].as_bool(), status["status"].as_str()),
        (Some(false), Some("deactivated"))
    );
    assert!(status.get("rev").is_none());
    let (st, v) = h
        .get("/xrpc/com.atproto.sync.getRepoStatus?did=did:plc:unknownunknownunknown".into())
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("RepoNotFound"))
    );
}

impl Harness {
    async fn raw_get(&self, path: String) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        self.raw("GET", &path, None, Vec::new(), &[]).await
    }
}

impl Harness {
    async fn import(&self, access: &str, car: Vec<u8>) -> (StatusCode, Value) {
        let (st, _, body) = self
            .raw(
                "POST",
                "/xrpc/com.atproto.repo.importRepo",
                Some(access),
                car,
                &[("content-type", "application/vnd.ipld.car")],
            )
            .await;
        (st, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    /// Polls getImportStatus until the latest import ends.
    async fn import_finished(&self, access: &str) -> Value {
        for _ in 0..200 {
            let (_, v) = self
                .call(
                    "GET",
                    "/xrpc/app.nearhorizon.locus.account.getImportStatus",
                    Some(access),
                    None,
                )
                .await;
            if v["import"]["done"] == true {
                return v["import"].clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("the import didn't finish");
    }

    async fn post_rkeys(&self, did: &str) -> Vec<(String, String)> {
        let (_, v) = self
            .get(format!(
                "/xrpc/com.atproto.repo.listRecords?repo={did}&collection=app.bsky.feed.post&reverse=true"
            ))
            .await;
        v["records"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["uri"]
                        .as_str()
                        .unwrap()
                        .rsplit('/')
                        .next()
                        .unwrap()
                        .to_owned(),
                    r["value"]["text"].as_str().unwrap_or_default().to_owned(),
                )
            })
            .collect()
    }
}

#[tokio::test]
async fn an_exported_repo_imports_back_as_a_diff() {
    let h = harness(false).await;
    let acct = h
        .create_account("ida.pds.example.com", "ida@example.com", "pw")
        .await;
    let (did, access) = (
        acct["did"].as_str().unwrap().to_owned(),
        acct["accessJwt"].as_str().unwrap().to_owned(),
    );
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend(std::iter::repeat_n(3u8, 500));
    let blob = h.upload(&access, &png, "image/png").await;
    let cid = blob["ref"]["$link"].as_str().unwrap().to_owned();
    let mut with_image = post_record("one");
    with_image["embed"] =
        json!({"$type": "app.bsky.embed.images", "images": [{"image": blob, "alt": ""}]});
    let put = |rkey: &'static str, record: Value| {
        h.post(
            "com.atproto.repo.putRecord",
            Some(&access),
            json!({"repo": did, "collection": "app.bsky.feed.post", "rkey": rkey, "record": record}),
        )
    };
    put("3m5aaaaaaaaaa", with_image).await;
    put("3m5aaaaaaaaab", post_record("two")).await;
    put("3m5aaaaaaaaac", post_record("three")).await;
    let (_, _, car) = h
        .raw_get(format!("/xrpc/com.atproto.sync.getRepo?did={did}"))
        .await;
    let (_, before) = h
        .get(format!("/xrpc/com.atproto.sync.getLatestCommit?did={did}"))
        .await;

    // Change things, then import the export: back to how it was.
    h.post(
        "com.atproto.repo.deleteRecord",
        Some(&access),
        json!({"repo": did, "collection": "app.bsky.feed.post", "rkey": "3m5aaaaaaaaaa"}),
    )
    .await;
    put("3m5aaaaaaaaac", post_record("THREE")).await;
    put("3m5aaaaaaaaad", post_record("four")).await;
    assert!(
        h.state
            .store
            .blob(&did, &cid)
            .await
            .unwrap()
            .unwrap()
            .unreferenced_since
            .is_some(),
        "the image's record went"
    );
    let (st, v) = h.import(&access, car.clone()).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let status = h.import_finished(&access).await;
    assert_eq!(status["phase"], "done", "{status}");
    assert!(status.get("notes").is_none(), "signed by the account's key");
    assert_eq!(
        h.post_rkeys(&did).await,
        vec![
            ("3m5aaaaaaaaaa".to_owned(), "one".to_owned()),
            ("3m5aaaaaaaaab".to_owned(), "two".to_owned()),
            ("3m5aaaaaaaaac".to_owned(), "three".to_owned()),
        ]
    );
    let row = h.state.store.blob(&did, &cid).await.unwrap().unwrap();
    assert_eq!(
        (row.state.as_str(), row.unreferenced_since),
        ("permanent", None)
    );

    // Re-signed with a newer rev over the same data, and verifiable.
    let signing = h.state.signing_key(&did).await.unwrap().did();
    let (_, _, now) = h
        .raw_get(format!("/xrpc/com.atproto.sync.getRepo?did={did}"))
        .await;
    let old = proto_blue_repo::sync::verify_repo_car(&car, Some(&did), Some(&signing)).unwrap();
    let new = proto_blue_repo::sync::verify_repo_car(&now, Some(&did), Some(&signing)).unwrap();
    assert_eq!(old.commit.data, new.commit.data);
    assert!(new.commit.rev.as_str() > before["rev"].as_str().unwrap());
    let kinds: Vec<String> = h
        .state
        .store
        .outbox_for(&did)
        .await
        .unwrap()
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(kinds.last().map(String::as_str), Some("sync"));
    let (_, missing) = h
        .call(
            "GET",
            "/xrpc/com.atproto.repo.listMissingBlobs",
            Some(&access),
            None,
        )
        .await;
    assert_eq!(missing["blobs"], json!([]));

    // Writes work again; imports can be turned off.
    let (st, _) = put("3m5aaaaaaaaae", post_record("five")).await;
    assert_eq!(st, StatusCode::OK);
    h.state
        .settings
        .set("accounts.accept_imports", json!(false), "test")
        .await
        .unwrap();
    let (st, v) = h.import(&access, car).await;
    assert_eq!(
        (st, v["message"].as_str()),
        (
            StatusCode::BAD_REQUEST,
            Some("Service is not accepting repo imports")
        )
    );
}

#[tokio::test]
async fn a_repo_from_elsewhere_lists_its_missing_blobs_until_uploaded() {
    use proto_blue_repo::commit::{SignedCommit, UnsignedCommit, sign_commit};
    let h = harness(false).await;
    let hr = &h;
    let make = |handle: &'static str| async move {
        let a = hr
            .create_account(
                &format!("{handle}.pds.example.com"),
                &format!("{handle}@example.com"),
                "pw",
            )
            .await;
        (
            a["did"].as_str().unwrap().to_owned(),
            a["accessJwt"].as_str().unwrap().to_owned(),
        )
    };
    let (src, src_access) = make("sora").await;
    let (dst, dst_access) = make("dana").await;
    let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];
    let blob = h.upload(&src_access, &png, "image/png").await;
    let cid = blob["ref"]["$link"].as_str().unwrap().to_owned();
    let mut record = post_record("moved");
    record["embed"] =
        json!({"$type": "app.bsky.embed.images", "images": [{"image": blob, "alt": ""}]});
    h.post(
        "com.atproto.repo.putRecord",
        Some(&src_access),
        json!({"repo": src, "collection": "app.bsky.feed.post", "rkey": "3m6aaaaaaaaaa", "record": record}),
    )
    .await;

    // The source's repo, re-signed for the destination by some other key.
    let (_, _, car) = h
        .raw_get(format!("/xrpc/com.atproto.sync.getRepo?did={src}"))
        .await;
    let (root, mut blocks) = proto_blue_repo::car::read_car_with_root(&car).unwrap();
    let commit = SignedCommit::from_lex_value(
        &proto_blue_lex_cbor::decode(blocks.get(&root).unwrap()).unwrap(),
    )
    .unwrap();
    let resigned = sign_commit(
        &UnsignedCommit::new(dst.clone(), commit.data.clone(), commit.rev.clone(), None),
        &proto_blue_crypto::K256Keypair::generate(),
    )
    .unwrap();
    let new_root = resigned.cid().unwrap();
    blocks.set(new_root.clone(), resigned.to_cbor().unwrap());
    let foreign = proto_blue_repo::car::blocks_to_car(Some(&new_root), &blocks).unwrap();

    // Refused as it is (another account's commit).
    let (st, v) = h.import(&dst_access, car).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(h.import_finished(&dst_access).await["phase"], "failed");

    let (st, v) = h.import(&dst_access, foreign).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let status = h.import_finished(&dst_access).await;
    assert_eq!(status["phase"], "done");
    assert!(
        status["notes"].as_str().unwrap().contains("re-signed"),
        "{status}"
    );
    assert_eq!(
        h.post_rkeys(&dst).await,
        vec![("3m6aaaaaaaaaa".to_owned(), "moved".to_owned())]
    );
    let (_, missing) = h
        .call(
            "GET",
            "/xrpc/com.atproto.repo.listMissingBlobs",
            Some(&dst_access),
            None,
        )
        .await;
    assert_eq!(
        missing,
        json!({
            "blobs": [{"cid": cid, "recordUri": format!("at://{dst}/app.bsky.feed.post/3m6aaaaaaaaaa")}],
            "cursor": cid,
        })
    );

    // Uploading it makes it permanent at once (a record already uses it).
    h.upload(&dst_access, &png, "image/png").await;
    let row = h.state.store.blob(&dst, &cid).await.unwrap().unwrap();
    assert_eq!(row.state, "permanent");
    assert!(
        row.storage_key.starts_with(&format!("blobs/{dst}/")),
        "{}",
        row.storage_key
    );
    let (_, missing) = h
        .call(
            "GET",
            "/xrpc/com.atproto.repo.listMissingBlobs",
            Some(&dst_access),
            None,
        )
        .await;
    assert_eq!(missing["blobs"], json!([]));
    let (st, _, body) = h
        .raw_get(format!(
            "/xrpc/com.atproto.sync.getBlob?did={dst}&cid={cid}"
        ))
        .await;
    assert_eq!((st, body), (StatusCode::OK, png.to_vec()));
}

/// What a mock upstream service saw.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    uri: String,
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
}

type SeenLog = Arc<Mutex<Vec<Seen>>>;

/// A service that records requests and answers by method name.
async fn mock_service() -> (String, SeenLog) {
    let log: SeenLog = Arc::default();
    let app = Router::new()
        .route(
            "/xrpc/{nsid}",
            axum::routing::any(
                |State(log): State<SeenLog>,
                 Path(nsid): Path<String>,
                 req: Request<Body>| async move {
                    let (parts, body) = req.into_parts();
                    let body = body.collect().await.unwrap().to_bytes().to_vec();
                    log.lock().unwrap().push(Seen {
                        method: parts.method.to_string(),
                        uri: parts.uri.to_string(),
                        headers: parts.headers.clone(),
                        body: body.clone(),
                    });
                    let mut res = match nsid.as_str() {
                        "app.bsky.fail.boom" => (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            axum::Json(json!({"error": "Boom", "message": "it broke"})),
                        )
                            .into_response(),
                        "app.bsky.fail.slow" => {
                            let mut r = (
                                StatusCode::TOO_MANY_REQUESTS,
                                axum::Json(json!({"error": "RateLimitExceeded", "message": "slow down"})),
                            )
                                .into_response();
                            r.headers_mut().insert("retry-after", "5".parse().unwrap());
                            r
                        }
                        "app.bsky.fail.plain" => (StatusCode::NOT_FOUND, "nope").into_response(),
                        _ if parts.method == "POST" => (
                            [("content-type", "application/json")],
                            body,
                        )
                            .into_response(),
                        _ => axum::Json(json!({"served": nsid})).into_response(),
                    };
                    res.headers_mut().insert("atproto-repo-rev", "3mrev".parse().unwrap());
                    res.headers_mut().insert("x-internal", "secret".parse().unwrap());
                    res
                },
            ),
        )
        .with_state(log.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), log)
}

#[tokio::test]
async fn unknown_methods_are_piped_through_to_the_configured_services() {
    let h = harness(false).await;
    let acct = h
        .create_account("pia.pds.example.com", "pia@example.com", "pw")
        .await;
    let (did, access) = (
        acct["did"].as_str().unwrap().to_owned(),
        acct["accessJwt"].as_str().unwrap().to_owned(),
    );
    let timeline = "/xrpc/app.bsky.feed.getTimeline?limit=5&cursor=a%20b";

    // Nothing configured yet.
    let (st, v) = h.call("GET", timeline, Some(&access), None).await;
    assert_eq!(
        (st, v["message"].as_str()),
        (
            StatusCode::BAD_REQUEST,
            Some("No service configured for app.bsky.feed.getTimeline")
        )
    );

    let (url, seen) = mock_service().await;
    for (k, v) in [
        ("appview.url", url.as_str()),
        ("appview.did", "did:web:appview.test"),
    ] {
        h.state.settings.set(k, json!(v), "test").await.unwrap();
    }

    // Forwarded with a service token from the account, and only the
    // reference's headers each way.
    let (st, headers, body) = h
        .raw(
            "GET",
            timeline,
            Some(&access),
            vec![],
            &[
                ("accept-language", "fr"),
                ("x-atproto-thing", "kept"),
                ("x-private", "dropped"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"served": "app.bsky.feed.getTimeline"})
    );
    assert_eq!(headers["atproto-repo-rev"], "3mrev");
    assert!(headers.get("x-internal").is_none());
    let req = seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(req.uri, timeline);
    assert_eq!(req.headers["accept-language"], "fr");
    assert_eq!(req.headers["x-atproto-thing"], "kept");
    assert!(req.headers.get("x-private").is_none());
    assert_eq!(req.headers["accept-encoding"], "identity");
    let token = req.headers["authorization"]
        .to_str()
        .unwrap()
        .strip_prefix("Bearer ")
        .unwrap()
        .to_owned();
    let jwt = locus_auth::service_jwt::parse(
        &token,
        Some("app.bsky.feed.getTimeline"),
        locus_core::time::now_ms() / 1000,
    )
    .unwrap();
    assert_eq!(
        (jwt.iss.as_str(), jwt.aud.as_str()),
        (did.as_str(), "did:web:appview.test")
    );
    let signing = h.state.signing_key(&did).await.unwrap().did();
    assert!(jwt.verifies_with(&signing));

    // Authentication is required; protected methods never leave.
    let (st, _) = h.call("GET", timeline, None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, v) = h
        .call(
            "GET",
            "/xrpc/com.atproto.server.GETSESSION",
            Some(&access),
            None,
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("InvalidToken"))
    );

    // Upstream errors: 500 becomes 502 with the upstream's name; others
    // pass through with retry-after.
    let (st, v) = h
        .call("GET", "/xrpc/app.bsky.fail.boom", Some(&access), None)
        .await;
    assert_eq!(
        (st, v),
        (
            StatusCode::BAD_GATEWAY,
            json!({"error": "Boom", "message": "it broke"})
        )
    );
    let (st, headers, _) = h
        .raw(
            "GET",
            "/xrpc/app.bsky.fail.slow",
            Some(&access),
            vec![],
            &[],
        )
        .await;
    assert_eq!(
        (st, headers["retry-after"].to_str().unwrap()),
        (StatusCode::TOO_MANY_REQUESTS, "5")
    );
    let (st, v) = h
        .call("GET", "/xrpc/app.bsky.fail.plain", Some(&access), None)
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("XRPCNotSupported"))
    );

    // Procedures stream their body; chat needs a privileged credential.
    let (st, v) = h
        .call(
            "POST",
            "/xrpc/chat.bsky.convo.sendMessage",
            Some(&access),
            Some(json!({"convoId": "c1", "message": {"text": "hi"}})),
        )
        .await;
    assert_eq!((st, v["convoId"].as_str()), (StatusCode::OK, Some("c1")));
    let req = seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        (
            req.method.as_str(),
            req.headers["content-type"].to_str().unwrap()
        ),
        ("POST", "application/json")
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&req.body).unwrap()["message"]["text"],
        "hi"
    );
    let (_, pw) = h
        .post(
            "com.atproto.server.createAppPassword",
            Some(&access),
            json!({"name": "plain"}),
        )
        .await;
    let (_, sess) = h
        .post(
            "com.atproto.server.createSession",
            None,
            json!({"identifier": did, "password": pw["password"]}),
        )
        .await;
    let app_access = sess["accessJwt"].as_str().unwrap().to_owned();
    let (st, v) = h
        .call(
            "POST",
            "/xrpc/chat.bsky.convo.sendMessage",
            Some(&app_access),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        (st, v["message"].as_str()),
        (StatusCode::BAD_REQUEST, Some("Bad token method"))
    );
    let (st, _) = h.call("GET", timeline, Some(&app_access), None).await;
    assert_eq!(st, StatusCode::OK, "app passwords reach the AppView");

    // atproto-proxy: the configured AppView by name, a malformed header, and
    // a DID whose endpoint is plain HTTP (refused by the SSRF rules).
    let (st, _, _) = h
        .raw(
            "GET",
            timeline,
            Some(&access),
            vec![],
            &[("atproto-proxy", "did:web:appview.test#bsky_appview")],
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, body) = h
        .raw(
            "GET",
            timeline,
            Some(&access),
            vec![],
            &[("atproto-proxy", "did:web:appview.test")],
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["message"],
        "no service id specified in proxy header"
    );
    let svc = "did:plc:svcsvcsvcsvcsvcsvcsvcsvc";
    h.plc.lock().unwrap().log.insert(
        svc.into(),
        vec![json!({
            "type": "plc_operation",
            "alsoKnownAs": [],
            "verificationMethods": {"atproto": proto_blue_crypto::K256Keypair::generate().did()},
            "services": {"labeler": {"type": "AtprotoLabeler", "endpoint": url}},
        })],
    );
    let (st, _, body) = h
        .raw(
            "GET",
            timeline,
            Some(&access),
            vec![],
            &[("atproto-proxy", &format!("{svc}#labeler"))],
        )
        .await;
    assert_eq!(
        st,
        StatusCode::BAD_GATEWAY,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let (st, _, _) = h
        .raw(
            "GET",
            timeline,
            Some(&access),
            vec![],
            &[("atproto-proxy", &format!("{svc}#nothing"))],
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // getRecord: local repos here, others from the AppView (no token).
    let (st, v) = h
        .get(
            "/xrpc/com.atproto.repo.getRecord?repo=did:plc:elsewhereelsewhereelse&collection=app.bsky.feed.post&rkey=x"
                .to_owned(),
        )
        .await;
    assert_eq!(
        (st, v["served"].as_str()),
        (StatusCode::OK, Some("com.atproto.repo.getRecord"))
    );
    assert!(
        seen.lock()
            .unwrap()
            .last()
            .unwrap()
            .headers
            .get("authorization")
            .is_none()
    );
    let (st, v) = h
        .get(format!(
            "/xrpc/com.atproto.repo.getRecord?repo={did}&collection=app.bsky.feed.post&rkey=x"
        ))
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("RecordNotFound"))
    );

    // An AppView that doesn't answer.
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    h.state
        .settings
        .set("appview.url", json!(dead), "test")
        .await
        .unwrap();
    let (st, v) = h.call("GET", timeline, Some(&access), None).await;
    assert_eq!(
        (st, v),
        (
            StatusCode::BAD_GATEWAY,
            json!({"error": "UpstreamFailure", "message": "Upstream service unreachable"})
        )
    );
}

#[tokio::test]
async fn preferences_replace_their_namespace_with_the_reference_rules() {
    let h = harness(false).await;
    let acct = h
        .create_account("pru.pds.example.com", "pru@example.com", "pw")
        .await;
    let (did, access) = (
        acct["did"].as_str().unwrap().to_owned(),
        acct["accessJwt"].as_str().unwrap().to_owned(),
    );
    let hr = &h;
    let get = |token: String| async move {
        hr.call(
            "GET",
            "/xrpc/app.bsky.actor.getPreferences",
            Some(&token),
            None,
        )
        .await
    };
    let put = |token: String, prefs: Value| async move {
        hr.call(
            "POST",
            "/xrpc/app.bsky.actor.putPreferences",
            Some(&token),
            Some(json!({ "preferences": prefs })),
        )
        .await
    };
    let (st, v) = get(access.clone()).await;
    assert_eq!((st, v), (StatusCode::OK, json!({"preferences": []})));

    let adult = json!({"$type": "app.bsky.actor.defs#adultContentPref", "enabled": true});
    let personal = json!({"$type": "app.bsky.actor.defs#personalDetailsPref", "birthDate": "2000-01-01T00:00:00.000Z"});
    let (st, _) = put(
        access.clone(),
        json!([adult, personal, {"$type": "app.bsky.actor.defs#declaredAgePref", "isOverAge18": false}]),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let age = json!({"$type": "app.bsky.actor.defs#declaredAgePref", "isOverAge13": true, "isOverAge16": true, "isOverAge18": true});
    let (_, v) = get(access.clone()).await;
    assert_eq!(
        v["preferences"],
        json!([adult, personal, age]),
        "stored in order, age derived"
    );

    // App passwords neither see nor touch personal details.
    let (_, pw) = h
        .post(
            "com.atproto.server.createAppPassword",
            Some(&access),
            json!({"name": "app"}),
        )
        .await;
    let (_, sess) = h
        .post(
            "com.atproto.server.createSession",
            None,
            json!({"identifier": did, "password": pw["password"]}),
        )
        .await;
    let app = sess["accessJwt"].as_str().unwrap().to_owned();
    let (_, v) = get(app.clone()).await;
    assert_eq!(v["preferences"], json!([adult, age]));
    let (st, v) = put(app.clone(), json!([personal])).await;
    assert_eq!(
        (st, v["message"].as_str()),
        (
            StatusCode::BAD_REQUEST,
            Some(
                "Do not have authorization to set preferences: app.bsky.actor.defs#personalDetailsPref"
            )
        )
    );
    let saved = json!({"$type": "app.bsky.actor.defs#savedFeedsPrefV2", "items": []});
    put(app.clone(), json!([saved])).await;
    let (_, v) = get(access.clone()).await;
    assert_eq!(
        v["preferences"],
        json!([personal, saved, age]),
        "personal details kept"
    );

    // Refusals.
    let (_, v) = put(access.clone(), json!([{"$type": "com.example.pref"}])).await;
    assert_eq!(
        v["message"],
        "Some preferences are not in the app.bsky namespace"
    );
    let (_, v) = put(access.clone(), json!([{"enabled": true}])).await;
    assert_eq!(v["message"], "Preference is missing a $type");

    // The moderation service reads any account's, with personal details.
    let mod_key = proto_blue_crypto::K256Keypair::generate();
    let mod_did = "did:plc:modmodmodmodmodmodmodmodm";
    h.plc.lock().unwrap().log.insert(
        mod_did.into(),
        vec![json!({
            "type": "plc_operation",
            "alsoKnownAs": [],
            "verificationMethods": {"atproto": mod_key.did()},
            "services": {},
        })],
    );
    let lxm = "app.bsky.actor.getPreferences";
    let mod_token = locus_auth::service_jwt::create(
        &mod_key,
        mod_did,
        &h.state.service_did,
        None,
        Some(lxm),
        locus_core::time::now_ms() / 1000,
    )
    .unwrap();
    let path = format!("/xrpc/{lxm}?did={did}");
    let (st, v) = h.call("GET", &path, Some(&mod_token), None).await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("AuthenticationRequired")),
        "{v}"
    );
    h.state
        .settings
        .set("moderation.service_did", json!(mod_did), "test")
        .await
        .unwrap();
    let (st, v) = h.call("GET", &path, Some(&mod_token), None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["preferences"], json!([personal, saved, age]));

    // atproto-proxy naming another service sends it there.
    let (url, seen) = mock_service().await;
    for (k, v) in [
        ("appview.url", url.as_str()),
        ("appview.did", "did:web:appview.test"),
    ] {
        h.state.settings.set(k, json!(v), "test").await.unwrap();
    }
    let (st, _, body) = h
        .raw(
            "GET",
            "/xrpc/app.bsky.actor.getPreferences",
            Some(&access),
            vec![],
            &[("atproto-proxy", "did:web:appview.test#bsky_appview")],
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["preferences"],
        json!([personal, saved, age])
    );
    assert!(
        seen.lock().unwrap().is_empty(),
        "the configured AppView's own name stays local"
    );
    let (st, _, _) = h
        .raw(
            "GET",
            "/xrpc/app.bsky.actor.getPreferences",
            Some(&access),
            vec![],
            &[("atproto-proxy", &format!("{mod_did}#bsky_appview"))],
        )
        .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "another service is resolved (none here)"
    );
}

/// A scripted AppView: per method, the status, extra headers and JSON body
/// it answers with. Records what it was sent.
type Script = Arc<Mutex<HashMap<String, (u16, Vec<(String, String)>, Value)>>>;

async fn scripted_appview() -> (String, Script, SeenLog) {
    let script: Script = Arc::default();
    let log: SeenLog = Arc::default();
    let app = Router::new()
        .route(
            "/xrpc/{nsid}",
            axum::routing::any(
                |State((script, log)): State<(Script, SeenLog)>,
                 Path(nsid): Path<String>,
                 req: Request<Body>| async move {
                    let (parts, body) = req.into_parts();
                    let body = body.collect().await.unwrap().to_bytes().to_vec();
                    log.lock().unwrap().push(Seen {
                        method: parts.method.to_string(),
                        uri: parts.uri.to_string(),
                        headers: parts.headers.clone(),
                        body,
                    });
                    // `nsid#cursor` answers a page of a listing.
                    let cursor = parts.uri.query().and_then(|q| {
                        url::form_urlencoded::parse(q.as_bytes())
                            .find(|(k, _)| k == "cursor")
                            .map(|(_, v)| v.into_owned())
                    });
                    let entry = {
                        let script = script.lock().unwrap();
                        cursor
                            .and_then(|c| script.get(&format!("{nsid}#{c}")).cloned())
                            .or_else(|| script.get(&nsid).cloned())
                    };
                    let Some((status, headers, body)) = entry else {
                        return (
                            StatusCode::NOT_IMPLEMENTED,
                            axum::Json(json!({"error": "MethodNotImplemented"})),
                        )
                            .into_response();
                    };
                    let mut res =
                        (StatusCode::from_u16(status).unwrap(), axum::Json(body)).into_response();
                    for (k, v) in headers {
                        res.headers_mut().insert(
                            axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                            v.parse().unwrap(),
                        );
                    }
                    res
                },
            ),
        )
        .with_state((script.clone(), log.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), script, log)
}

#[tokio::test]
async fn the_appview_s_answers_include_what_was_just_written_here() {
    let h = harness(false).await;
    let acct = h
        .create_account("rae.pds.example.com", "rae@example.com", "pw")
        .await;
    let (did, access) = (
        acct["did"].as_str().unwrap().to_owned(),
        acct["accessJwt"].as_str().unwrap().to_owned(),
    );
    let (url, script, seen) = scripted_appview().await;
    for (k, v) in [
        ("appview.url", url.as_str()),
        ("appview.did", "did:web:appview.test"),
    ] {
        h.state.settings.set(k, json!(v), "test").await.unwrap();
    }
    let create = |collection: &'static str, rkey: &'static str, record: Value| {
        let (h, access, did) = (&h, access.clone(), did.clone());
        async move {
            let (st, v) = h
                .post(
                    "com.atproto.repo.putRecord",
                    Some(&access),
                    json!({"repo": did, "collection": collection, "rkey": rkey, "record": record}),
                )
                .await;
            assert_eq!(st, StatusCode::OK, "{v}");
            v
        }
    };
    create(
        "app.bsky.feed.post",
        "3m7aaaaaaaaaa",
        post_record("seen by the appview"),
    )
    .await;
    let (_, latest) = h
        .get(format!("/xrpc/com.atproto.sync.getLatestCommit?did={did}"))
        .await;
    let seen_rev = latest["rev"].as_str().unwrap().to_owned();

    // Written after the rev the AppView has: a profile edit, a post that
    // quotes another, and a reply to that post.
    create(
        "app.bsky.actor.profile",
        "self",
        json!({"$type": "app.bsky.actor.profile", "displayName": "New Name", "description": "new"}),
    )
    .await;
    let quoted = "at://did:plc:otherotherotherotherothe/app.bsky.feed.post/3m7qqqqqqqqqq";
    let mut p1 = post_record("fresh");
    p1["embed"] = json!({"$type": "app.bsky.embed.record", "record": {"uri": quoted, "cid": "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifvx5xytvske7mr3hpm"}});
    let p1v = create("app.bsky.feed.post", "3m7bbbbbbbbbb", p1).await;
    let p1_uri = p1v["uri"].as_str().unwrap().to_owned();
    let mut p2 = post_record("reply");
    p2["reply"] = json!({"root": {"uri": p1_uri, "cid": p1v["cid"]}, "parent": {"uri": p1_uri, "cid": p1v["cid"]}});
    let p2_uri = create("app.bsky.feed.post", "3m7cccccccccc", p2).await["uri"]
        .as_str()
        .unwrap()
        .to_owned();

    let rev = vec![("atproto-repo-rev".to_owned(), seen_rev.clone())];
    let me = json!({"did": did, "handle": "rae.pds.example.com", "displayName": "Old", "description": "old", "banner": "x"});
    let other_post = json!({"post": {"uri": "at://did:plc:x/app.bsky.feed.post/1", "cid": "c", "indexedAt": "2000-01-01T00:00:00.000Z", "author": {"did": "did:plc:x", "handle": "x.test"}}});
    {
        let mut sc = script.lock().unwrap();
        sc.insert(
            "app.bsky.actor.getProfile".into(),
            (200, rev.clone(), me.clone()),
        );
        sc.insert(
            "app.bsky.feed.getTimeline".into(),
            (
                200,
                rev.clone(),
                json!({"feed": [other_post], "cursor": "c1"}),
            ),
        );
        sc.insert(
            "app.bsky.feed.getPosts".into(),
            (200, vec![], json!({"posts": [{"uri": quoted, "cid": "qc", "author": {"did": "did:plc:otherotherotherotherothe", "handle": "o.test"}, "record": {"text": "quoted"}, "indexedAt": "2001-01-01T00:00:00.000Z"}]})),
        );
        sc.insert(
            "app.bsky.feed.getPostThread".into(),
            (
                400,
                rev.clone(),
                json!({"error": "NotFound", "message": "Post not found"}),
            ),
        );
    }

    // The profile has the new name and description; the banner is gone.
    let (st, headers, body) = h
        .raw(
            "GET",
            &format!("/xrpc/app.bsky.actor.getProfile?actor={did}"),
            Some(&access),
            vec![],
            &[],
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        v,
        json!({"did": did, "handle": "rae.pds.example.com", "displayName": "New Name", "description": "new"})
    );
    assert!(headers.get("atproto-upstream-lag").is_some());

    // The timeline gets the two new posts first, newest first, with the
    // quote resolved through the AppView.
    let (_, v) = h
        .call(
            "GET",
            "/xrpc/app.bsky.feed.getTimeline",
            Some(&access),
            None,
        )
        .await;
    let uris: Vec<&str> = v["feed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["post"]["uri"].as_str().unwrap())
        .collect();
    assert_eq!(
        uris,
        vec![
            p2_uri.as_str(),
            p1_uri.as_str(),
            "at://did:plc:x/app.bsky.feed.post/1"
        ]
    );
    assert_eq!(v["cursor"], "c1");
    let p1_view = &v["feed"][1]["post"];
    assert_eq!(
        p1_view["author"],
        json!({"did": did, "handle": "rae.pds.example.com", "displayName": "New Name"})
    );
    assert_eq!(p1_view["likeCount"], 0);
    assert_eq!(
        p1_view["embed"],
        json!({"$type": "app.bsky.embed.record#view", "record": {
            "$type": "app.bsky.embed.record#viewRecord", "uri": quoted, "cid": "qc",
            "author": {"did": "did:plc:otherotherotherotherothe", "handle": "o.test"},
            "value": {"text": "quoted"}, "indexedAt": "2001-01-01T00:00:00.000Z"}})
    );
    let getposts = seen
        .lock()
        .unwrap()
        .iter()
        .find(|r| r.uri.starts_with("/xrpc/app.bsky.feed.getPosts"))
        .cloned()
        .unwrap();
    assert!(
        getposts.headers.get("authorization").is_some(),
        "as the requester"
    );

    // A thread the AppView hasn't seen yet is built here.
    let (st, v) = h
        .call(
            "GET",
            &format!(
                "/xrpc/app.bsky.feed.getPostThread?uri={}",
                p1_uri.replace(':', "%3A")
            ),
            Some(&access),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["thread"]["post"]["uri"], p1_uri.as_str());
    assert_eq!(v["thread"]["replies"][0]["post"]["uri"], p2_uri.as_str());
    // Someone else's missing post stays missing.
    let (st, v) = h
        .call(
            "GET",
            "/xrpc/app.bsky.feed.getPostThread?uri=at%3A%2F%2Fdid%3Aplc%3Ax%2Fapp.bsky.feed.post%2F1",
            Some(&access),
            None,
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("NotFound"))
    );

    // No rev, or a rev that has everything: passed through as is.
    script.lock().unwrap().insert(
        "app.bsky.actor.getProfile".into(),
        (200, vec![], me.clone()),
    );
    let (_, v) = h
        .call(
            "GET",
            "/xrpc/app.bsky.actor.getProfile?actor=x",
            Some(&access),
            None,
        )
        .await;
    assert_eq!(v, me);
    let (_, latest) = h
        .get(format!("/xrpc/com.atproto.sync.getLatestCommit?did={did}"))
        .await;
    script.lock().unwrap().insert(
        "app.bsky.actor.getProfile".into(),
        (
            200,
            vec![(
                "atproto-repo-rev".into(),
                latest["rev"].as_str().unwrap().into(),
            )],
            me.clone(),
        ),
    );
    let (_, v) = h
        .call(
            "GET",
            "/xrpc/app.bsky.actor.getProfile?actor=x",
            Some(&access),
            None,
        )
        .await;
    assert_eq!(v, me);

    // getFeed: the generator's DID from its record, then a token for it.
    script.lock().unwrap().insert(
        "com.atproto.repo.getRecord".into(),
        (200, vec![], json!({"uri": "at://did:plc:fg/app.bsky.feed.generator/cool", "value": {"did": "did:web:feedgen.test"}})),
    );
    script.lock().unwrap().insert(
        "app.bsky.feed.getFeed".into(),
        (200, vec![], json!({"feed": []})),
    );
    let (st, v) = h
        .call(
            "GET",
            "/xrpc/app.bsky.feed.getFeed?feed=at%3A%2F%2Fdid%3Aplc%3Afg%2Fapp.bsky.feed.generator%2Fcool",
            Some(&access),
            None,
        )
        .await;
    assert_eq!((st, v), (StatusCode::OK, json!({"feed": []})));
    let req = seen.lock().unwrap().last().cloned().unwrap();
    let token = req.headers["authorization"].to_str().unwrap()[7..].to_owned();
    let jwt = locus_auth::service_jwt::parse(
        &token,
        Some("app.bsky.feed.getFeedSkeleton"),
        locus_core::time::now_ms() / 1000,
    )
    .unwrap();
    assert_eq!(jwt.aud, "did:web:feedgen.test");

    // registerPush to the AppView, as the account.
    script.lock().unwrap().insert(
        "app.bsky.notification.registerPush".into(),
        (200, vec![], json!({})),
    );
    let (st, _) = h
        .call(
            "POST",
            "/xrpc/app.bsky.notification.registerPush",
            Some(&access),
            Some(json!({"serviceDid": "did:web:appview.test", "token": "t", "platform": "ios", "appId": "x"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let req = seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&req.body).unwrap()["token"],
        "t"
    );
    let token = req.headers["authorization"].to_str().unwrap()[7..].to_owned();
    let jwt = locus_auth::service_jwt::parse(
        &token,
        Some("app.bsky.notification.registerPush"),
        locus_core::time::now_ms() / 1000,
    )
    .unwrap();
    assert_eq!(
        (jwt.iss.as_str(), jwt.aud.as_str()),
        (did.as_str(), "did:web:appview.test")
    );
}

impl Harness {
    /// A request from `peer` (as the connection's address).
    async fn call_from(
        &self,
        peer: &str,
        method: &str,
        path: &str,
        auth: Option<&str>,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> (StatusCode, axum::http::HeaderMap, Value) {
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
        let mut req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let addr: std::net::SocketAddr = format!("{peer}:40000").parse().unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(addr));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let (status, headers) = (res.status(), res.headers().clone());
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            headers,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

#[tokio::test]
async fn rate_limits_count_by_real_client_ip_and_account() {
    for backend in ["memory", "database"] {
        let h = harness(false).await;
        let set = |k: &'static str, v: Value| {
            let h = &h;
            async move { h.state.settings.set(k, v, "test").await.unwrap() }
        };
        set("rate_limits.backend", json!(backend)).await;
        set("rate_limits.global_points", json!(3)).await;
        let describe = "/xrpc/com.atproto.server.describeServer";

        // The global bucket: three requests per client, then 429.
        for remaining in ["2", "1", "0"] {
            let (st, headers, _) = h
                .call_from("203.0.113.9", "GET", describe, None, None, &[])
                .await;
            assert_eq!(st, StatusCode::OK, "{backend}");
            assert_eq!(headers["ratelimit-remaining"], remaining, "{backend}");
            assert_eq!(headers["ratelimit-limit"], "3");
            assert_eq!(headers["ratelimit-policy"], "3;w=300");
        }
        let (st, headers, v) = h
            .call_from("203.0.113.9", "GET", describe, None, None, &[])
            .await;
        assert_eq!(
            (st, v["error"].as_str(), v["message"].as_str()),
            (
                StatusCode::TOO_MANY_REQUESTS,
                Some("RateLimitExceeded"),
                Some("Rate Limit Exceeded")
            ),
            "{backend}"
        );
        let retry: i64 = headers["retry-after"].to_str().unwrap().parse().unwrap();
        assert!((1..=300).contains(&retry), "{retry}");
        // Another client has its own bucket; health checks never count.
        let (st, _, _) = h
            .call_from("203.0.113.10", "GET", describe, None, None, &[])
            .await;
        assert_eq!(st, StatusCode::OK);
        let (st, _, _) = h
            .call_from("203.0.113.9", "GET", "/xrpc/_health", None, None, &[])
            .await;
        assert_eq!(st, StatusCode::OK);

        // Forwarded headers count only from a trusted proxy (loopback).
        for _ in 0..3 {
            h.call_from(
                "127.0.0.1",
                "GET",
                describe,
                None,
                None,
                &[("x-forwarded-for", "198.51.100.1")],
            )
            .await;
        }
        let (st, _, _) = h
            .call_from(
                "127.0.0.1",
                "GET",
                describe,
                None,
                None,
                &[("x-forwarded-for", "198.51.100.1")],
            )
            .await;
        assert_eq!(
            st,
            StatusCode::TOO_MANY_REQUESTS,
            "{backend}: keyed by the forwarded client"
        );
        let (st, _, _) = h
            .call_from(
                "127.0.0.1",
                "GET",
                describe,
                None,
                None,
                &[("x-forwarded-for", "198.51.100.2, 127.0.0.1")],
            )
            .await;
        assert_eq!(st, StatusCode::OK, "the right-most untrusted address");
        let (st, _, _) = h
            .call_from(
                "203.0.113.9",
                "GET",
                describe,
                None,
                None,
                &[("x-forwarded-for", "198.51.100.3")],
            )
            .await;
        assert_eq!(
            st,
            StatusCode::TOO_MANY_REQUESTS,
            "an untrusted peer can't pick its address"
        );

        // Bypass by header or by address.
        set("rate_limits.bypass_key", json!("let-me-in")).await;
        set("rate_limits.bypass_ips", json!(["203.0.113.0/24"])).await;
        let (st, _, _) = h
            .call_from(
                "198.51.100.1",
                "GET",
                describe,
                None,
                None,
                &[("x-ratelimit-bypass", "let-me-in")],
            )
            .await;
        assert_eq!(st, StatusCode::OK);
        let (st, _, _) = h
            .call_from("203.0.113.9", "GET", describe, None, None, &[])
            .await;
        assert_eq!(st, StatusCode::OK);
        set("rate_limits.bypass_ips", json!([])).await;
        set("rate_limits.global_points", json!(3000)).await;

        // Repo writes: points per account (create 3, put 2, delete 1).
        set("rate_limits.repo_write_hour_points", json!(5)).await;
        let acct = h
            .create_account("rlu.pds.example.com", "rlu@example.com", "pw")
            .await;
        let (did, access) = (
            acct["did"].as_str().unwrap().to_owned(),
            acct["accessJwt"].as_str().unwrap().to_owned(),
        );
        let write = |nsid: &'static str, body: Value| {
            let (h, access) = (&h, access.clone());
            async move {
                h.call_from(
                    "192.0.2.1",
                    "POST",
                    &format!("/xrpc/{nsid}"),
                    Some(&access),
                    Some(body),
                    &[],
                )
                .await
            }
        };
        let rec = json!({"repo": did, "collection": "app.bsky.feed.post", "rkey": "3m6aaaaaaaaab", "record": post_record("x")});
        let (st, h1, v) = write("com.atproto.repo.createRecord", rec.clone()).await;
        assert_eq!(
            (st, h1["ratelimit-remaining"].to_str().unwrap()),
            (StatusCode::OK, "2"),
            "{backend}: {v}"
        );
        let (st, _, _) = write("com.atproto.repo.putRecord", rec.clone()).await;
        assert_eq!(st, StatusCode::OK);
        let del = json!({"repo": did, "collection": "app.bsky.feed.post", "rkey": "3m6aaaaaaaaab"});
        let (st, _, v) = write("com.atproto.repo.deleteRecord", del).await;
        assert_eq!(
            (st, v["error"].as_str()),
            (StatusCode::TOO_MANY_REQUESTS, Some("RateLimitExceeded")),
            "{backend}"
        );
    }
}

#[tokio::test]
async fn firehose_connections_are_limited_per_ip() {
    let h = harness(false).await;
    h.state
        .settings
        .set("sync.max_subscribers_per_ip", json!(1), "test")
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = h
        .app
        .clone()
        .into_make_service_with_connect_info::<std::net::SocketAddr>();
    tokio::spawn(axum::serve(listener, app).into_future());
    let url = format!("ws://{addr}/xrpc/com.atproto.sync.subscribeRepos");
    let req = |u: &str| {
        let mut r =
            tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(u)
                .unwrap();
        r.headers_mut()
            .insert("host", h.state.hostname.parse().unwrap());
        r
    };
    let (first, _) = tokio_tungstenite::connect_async(req(&url)).await.unwrap();
    let second = tokio_tungstenite::connect_async(req(&url)).await;
    match second {
        Err(tokio_tungstenite::tungstenite::Error::Http(res)) => {
            assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
        }
        other => panic!("expected a refusal, got {:?}", other.map(|_| ())),
    }
    // A place frees when its connection closes.
    drop(first);
    let mut ok = false;
    for _ in 0..50 {
        if tokio_tungstenite::connect_async(req(&url)).await.is_ok() {
            ok = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(ok, "the place was released");
}

#[tokio::test]
async fn cors_cache_headers_and_the_request_deadline() {
    let h = harness(false).await;
    let acct = h
        .create_account("cor.pds.example.com", "cor@example.com", "pw")
        .await;
    let access = acct["accessJwt"].as_str().unwrap().to_owned();

    // Preflight, as the reference's cors().
    let (st, headers, _) = h
        .raw(
            "OPTIONS",
            "/xrpc/com.atproto.server.getSession",
            None,
            vec![],
            &[
                ("origin", "https://app.example"),
                ("access-control-request-method", "GET"),
                (
                    "access-control-request-headers",
                    "authorization,atproto-proxy",
                ),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert_eq!(headers["access-control-allow-origin"], "*");
    assert_eq!(
        headers["access-control-allow-headers"],
        "authorization,atproto-proxy"
    );
    assert_eq!(headers["access-control-max-age"], "86400");

    // Every response allows any origin; authenticated ones aren't shared.
    let (_, headers, _) = h
        .raw(
            "GET",
            "/xrpc/com.atproto.server.describeServer",
            None,
            vec![],
            &[],
        )
        .await;
    assert_eq!(headers["access-control-allow-origin"], "*");
    assert!(headers.get("cache-control").is_none());
    let (_, headers, _) = h
        .raw(
            "GET",
            "/xrpc/com.atproto.server.getSession",
            Some(&access),
            vec![],
            &[],
        )
        .await;
    assert_eq!(headers["cache-control"], "private");
    assert!(headers.get_all("vary").iter().any(|v| v == "Authorization"));

    // A request that outlasts the deadline gets a retryable error.
    let slow = Router::new().route(
        "/xrpc/{nsid}",
        get(|| async {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            "late"
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, slow).await.unwrap() });
    for (k, v) in [
        ("appview.url", json!(url)),
        ("appview.did", json!("did:web:appview.test")),
        ("http.request_deadline_ms", json!(1000)),
    ] {
        h.state.settings.set(k, v, "test").await.unwrap();
    }
    let started = std::time::Instant::now();
    let (st, headers, body) = h
        .raw(
            "GET",
            "/xrpc/app.bsky.unspecced.slow",
            Some(&access),
            vec![],
            &[],
        )
        .await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(headers["retry-after"], "1");
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["error"],
        "RequestTimeout"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
}

#[tokio::test]
async fn oauth_metadata_and_jwks() {
    let h = harness(false).await;
    let (st, md) = h
        .get("/.well-known/oauth-authorization-server".into())
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(md["issuer"], "https://pds.example.com");
    assert_eq!(
        md["pushed_authorization_request_endpoint"],
        "https://pds.example.com/oauth/par"
    );
    assert_eq!(md["require_pushed_authorization_requests"], true);
    assert_eq!(md["code_challenge_methods_supported"], json!(["S256"]));
    assert_eq!(
        md["protected_resources"],
        json!(["https://pds.example.com"])
    );
    let (_, pr) = h.get("/.well-known/oauth-protected-resource".into()).await;
    assert_eq!(pr["resource"], "https://pds.example.com");
    assert_eq!(
        pr["authorization_servers"],
        json!(["https://pds.example.com"])
    );
    let (_, jwks) = h.get("/oauth/jwks".into()).await;
    let key = &jwks["keys"][0];
    assert_eq!(
        (
            key["kty"].as_str(),
            key["crv"].as_str(),
            key["alg"].as_str()
        ),
        (Some("EC"), Some("P-256"), Some("ES256"))
    );
    assert!(key.get("d").is_none(), "public only");
    assert_eq!(key["kid"], h.state.secrets.oauth_key.kid());
}

#[tokio::test]
async fn oauth_clients_resolve_from_metadata_or_loopback_ids() {
    let h = harness(false).await;
    let lb = crate::oauth::get_client(&h.state, "http://localhost")
        .await
        .unwrap();
    assert_eq!(lb.metadata.application_type, "native");
    assert!(!lb.is_confidential());
    let id = "https://app.example.org/client-metadata.json";
    let md = json!({
        "client_id": id,
        "client_name": "Example",
        "scope": "atproto transition:generic",
        "redirect_uris": ["https://app.example.org/callback"],
        "grant_types": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_method": "none",
        "dpop_bound_access_tokens": true,
    });
    h.state.oauth_clients.seed(id, md.clone());
    let c = crate::oauth::get_client(&h.state, id).await.unwrap();
    assert_eq!(c.metadata.client_name(), Some("Example"));
    assert!(!c.is_trusted);
    h.state
        .settings
        .set("oauth.trusted_clients", json!([id]), "test")
        .await
        .unwrap();
    assert!(
        crate::oauth::get_client(&h.state, id)
            .await
            .unwrap()
            .is_trusted
    );
    // Bad metadata and ids are refused.
    let bad = "https://bad.example.org/client-metadata.json";
    let mut m = md.clone();
    m["client_id"] = json!(bad);
    m["dpop_bound_access_tokens"] = json!(false);
    h.state.oauth_clients.seed(bad, m);
    let e = crate::oauth::get_client(&h.state, bad).await.unwrap_err();
    assert_eq!(e.error, "invalid_client_metadata");
    assert!(
        crate::oauth::get_client(&h.state, "not-a-client")
            .await
            .is_err()
    );
    // An unreachable (here: IP-literal, refused) metadata URL.
    let e = crate::oauth::get_client(&h.state, "https://unreachable.invalid.example.com/m.json")
        .await
        .unwrap_err();
    assert!(
        e.description
            .starts_with("Unable to obtain client metadata"),
        "{}",
        e.description
    );
}

/// An OAuth client for tests: a DPoP key and the nonce it last got.
struct DpopClient {
    key: locus_auth::oauth::jose::Es256Key,
    nonce: Mutex<Option<String>>,
}

impl DpopClient {
    fn new() -> Self {
        Self {
            key: locus_auth::oauth::jose::Es256Key::generate(),
            nonce: Mutex::new(None),
        }
    }

    fn jkt(&self) -> String {
        let mut jwk = self.key.jwk();
        for k in ["kid", "use", "alg"] {
            jwk.as_object_mut().unwrap().remove(k);
        }
        locus_auth::oauth::jose::thumbprint(jwk.as_object().unwrap()).unwrap()
    }

    fn proof(&self, method: &str, url: &str, access_token: Option<&str>) -> String {
        use sha2::Digest;
        let mut jwk = self.key.jwk();
        for k in ["kid", "use", "alg"] {
            jwk.as_object_mut().unwrap().remove(k);
        }
        let mut header = serde_json::Map::new();
        header.insert("typ".into(), json!("dpop+jwt"));
        header.insert("jwk".into(), jwk);
        let mut claims = json!({
            "jti": hex::encode(rand::random::<[u8; 12]>()),
            "htm": method,
            "htu": url,
            "iat": locus_core::time::now_ms() / 1000,
        });
        if let Some(n) = self.nonce.lock().unwrap().clone() {
            claims["nonce"] = json!(n);
        }
        if let Some(t) = access_token {
            claims["ath"] = json!(locus_auth::oauth::jose::b64(&sha2::Sha256::digest(
                t.as_bytes()
            )));
        }
        self.key.sign(header, &claims)
    }

    /// A form POST to an OAuth endpoint, retried once if a nonce is asked for.
    async fn post(
        &self,
        h: &Harness,
        path: &str,
        form: &[(&str, &str)],
    ) -> (StatusCode, axum::http::HeaderMap, Value) {
        let body: String = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form.iter().copied())
            .finish();
        for _ in 0..2 {
            let proof = self.proof("POST", &format!("https://pds.example.com{path}"), None);
            let (st, headers, bytes) = h
                .raw(
                    "POST",
                    path,
                    None,
                    body.clone().into_bytes(),
                    &[
                        ("content-type", "application/x-www-form-urlencoded"),
                        ("dpop", &proof),
                    ],
                )
                .await;
            if let Some(n) = headers.get("dpop-nonce") {
                *self.nonce.lock().unwrap() = Some(n.to_str().unwrap().to_owned());
            }
            let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            if v["error"] == "use_dpop_nonce" {
                continue;
            }
            return (st, headers, v);
        }
        panic!("the nonce retry didn't succeed");
    }
}

fn pkce() -> (String, String) {
    use sha2::Digest;
    let verifier = hex::encode(rand::random::<[u8; 32]>());
    let challenge = locus_auth::oauth::jose::b64(&sha2::Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

#[tokio::test]
async fn oauth_par_code_token_refresh_and_revoke() {
    const LB: &str = "http://localhost?scope=atproto+transition%3Ageneric";
    let h = harness(false).await;
    let acct = h
        .create_account("oli.pds.example.com", "oli@example.com", "pw")
        .await;
    let did = acct["did"].as_str().unwrap().to_owned();
    let client = DpopClient::new();
    let (verifier, challenge) = pkce();
    let redirect = "http://127.0.0.1:5173/";

    // The first call has no nonce: the server asks for one.
    let first = client.proof("POST", "https://pds.example.com/oauth/par", None);
    let (st, headers, v) = h
        .raw(
            "POST",
            "/oauth/par",
            None,
            b"client_id=http%3A%2F%2Flocalhost&response_type=code".to_vec(),
            &[
                ("content-type", "application/x-www-form-urlencoded"),
                ("dpop", &first),
            ],
        )
        .await;
    let v: Value = serde_json::from_slice(&v).unwrap();
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("use_dpop_nonce"))
    );
    assert!(headers.contains_key("dpop-nonce"));
    assert_eq!(headers["cache-control"], "no-store");

    let par = |extra: &'static [(&'static str, &'static str)], challenge: String| {
        let client = &client;
        let h = &h;
        async move {
            let mut form = vec![
                ("client_id", LB),
                ("response_type", "code"),
                ("scope", "atproto transition:generic"),
                ("redirect_uri", redirect),
                ("code_challenge_method", "S256"),
                ("state", "s1"),
            ];
            form.push(("code_challenge", &challenge));
            form.extend_from_slice(extra);
            client.post(h, "/oauth/par", &form).await
        }
    };
    let (st, _, v) = par(&[], challenge.clone()).await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    let request_uri = v["request_uri"].as_str().unwrap().to_owned();
    assert_eq!(v["expires_in"], 300);
    let (st, _, v) = par(&[("prompt", "none")], challenge.clone()).await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_request")),
        "consent_required becomes invalid_request at PAR"
    );

    // The sign-in page's steps: open the request on a device, approve it.
    let id = locus_auth::oauth::request::request_id_from_uri(&request_uri)
        .unwrap()
        .to_owned();
    let (_, p) = crate::oauth::open_request(&h.state, &id, "dev-test", LB)
        .await
        .unwrap();
    assert_eq!(
        p.dpop_jkt.as_deref(),
        Some(client.jkt().as_str()),
        "bound to the PAR's DPoP key"
    );
    assert!(
        crate::oauth::consent_required(&h.state, &did, &p)
            .await
            .unwrap()
    );
    let (code, _) = crate::oauth::authorize(
        &h.state,
        &id,
        "dev-test",
        &did,
        Some("atproto transition:generic"),
    )
    .await
    .unwrap();

    // Exchange the code (a wrong verifier first fails without spending it?
    // no: the code is consumed either way, as the reference).
    let (st, _, tok) = client
        .post(
            &h,
            "/oauth/token",
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", redirect),
                ("code_verifier", &verifier),
                ("client_id", LB),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{tok}");
    assert_eq!(
        (tok["token_type"].as_str(), tok["sub"].as_str()),
        (Some("DPoP"), Some(did.as_str()))
    );
    assert_eq!(tok["scope"], "atproto transition:generic");
    let access = tok["access_token"].as_str().unwrap().to_owned();
    let refresh = tok["refresh_token"].as_str().unwrap().to_owned();
    let jws = locus_auth::oauth::jose::Jws::decode(&access).unwrap();
    h.state.secrets.oauth_key.verify(&jws).unwrap();
    assert_eq!(jws.payload["cnf"]["jkt"], client.jkt());
    assert_eq!(jws.payload["aud"], "did:web:pds.example.com");
    assert!(jws.typ_is("at+jwt"));

    // A replayed code is refused and revokes what it was exchanged for.
    let (st, _, v) = client
        .post(
            &h,
            "/oauth/token",
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", redirect),
                ("code_verifier", &verifier),
                ("client_id", LB),
            ],
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_grant"))
    );
    assert!(
        h.state
            .store
            .oauth_token_by_code(&code)
            .await
            .unwrap()
            .is_none(),
        "revoked"
    );

    // A second grant to test refresh rotation and replay.
    let (verifier, challenge) = pkce();
    let (_, _, v) = par(&[], challenge).await;
    let id = locus_auth::oauth::request::request_id_from_uri(v["request_uri"].as_str().unwrap())
        .unwrap()
        .to_owned();
    crate::oauth::open_request(&h.state, &id, "dev-test", LB)
        .await
        .unwrap();
    let (code, _) = crate::oauth::authorize(&h.state, &id, "dev-test", &did, Some("atproto"))
        .await
        .unwrap();
    let (_, _, tok) = client
        .post(
            &h,
            "/oauth/token",
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", redirect),
                ("code_verifier", &verifier),
                ("client_id", LB),
            ],
        )
        .await;
    assert_eq!(tok["scope"], "atproto", "only what was approved");
    let _ = (access, refresh);
    let r1 = tok["refresh_token"].as_str().unwrap().to_owned();
    let (st, _, tok2) = client
        .post(
            &h,
            "/oauth/token",
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &r1),
                ("client_id", LB),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{tok2}");
    let r2 = tok2["refresh_token"].as_str().unwrap().to_owned();
    assert_ne!(r1, r2);
    let (_, _, v) = client
        .post(
            &h,
            "/oauth/token",
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &r1),
                ("client_id", LB),
            ],
        )
        .await;
    assert_eq!(v["error_description"], "Refresh token replayed");
    let (_, _, v) = client
        .post(
            &h,
            "/oauth/token",
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &r2),
                ("client_id", LB),
            ],
        )
        .await;
    assert_eq!(v["error"], "invalid_grant", "the replay ended the session");

    // Another key can't use the grant; revoke always answers {}.
    let (verifier, challenge) = pkce();
    let (_, _, v) = par(&[], challenge).await;
    let id = locus_auth::oauth::request::request_id_from_uri(v["request_uri"].as_str().unwrap())
        .unwrap()
        .to_owned();
    crate::oauth::open_request(&h.state, &id, "dev-test", LB)
        .await
        .unwrap();
    let (code, _) = crate::oauth::authorize(&h.state, &id, "dev-test", &did, Some("atproto"))
        .await
        .unwrap();
    let thief = DpopClient::new();
    let (_, _, v) = thief
        .post(
            &h,
            "/oauth/token",
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", redirect),
                ("code_verifier", &verifier),
                ("client_id", LB),
            ],
        )
        .await;
    assert_eq!(
        v["error_description"],
        "DPoP proof does not match the expected JKT"
    );
    let (st, _, v) = client
        .post(
            &h,
            "/oauth/revoke",
            &[("token", "ref-nonexistent"), ("client_id", LB)],
        )
        .await;
    assert_eq!((st, v), (StatusCode::OK, json!({})));
    // A request can't be opened twice once authorized, or from another device.
    let (_verifier, challenge) = pkce();
    let (_, _, v) = par(&[], challenge).await;
    let id = locus_auth::oauth::request::request_id_from_uri(v["request_uri"].as_str().unwrap())
        .unwrap()
        .to_owned();
    crate::oauth::open_request(&h.state, &id, "dev-a", LB)
        .await
        .unwrap();
    let e = crate::oauth::open_request(&h.state, &id, "dev-b", LB)
        .await
        .unwrap_err();
    assert_eq!(
        e.description,
        "This request was initiated from another device"
    );
    let e = crate::oauth::open_request(&h.state, &id, "dev-a", LB)
        .await
        .unwrap_err();
    assert_eq!(
        e.description, "Unknown request_uri",
        "deleted by the refusal"
    );
}

#[tokio::test]
async fn include_scopes_expand_from_a_published_permission_set() {
    let h = harness(false).await;
    let acct = h
        .create_account("lexi.pds.example.com", "lexi@example.com", "pw")
        .await;
    let (did, access) = (
        acct["did"].as_str().unwrap().to_owned(),
        acct["accessJwt"].as_str().unwrap().to_owned(),
    );
    let nsid = "com.example.cal.auth";
    let (st, v) = h
        .post(
            "com.atproto.repo.putRecord",
            Some(&access),
            json!({"repo": did, "collection": "com.atproto.lexicon.schema", "rkey": nsid, "record": {
                "$type": "com.atproto.lexicon.schema",
                "lexicon": 1,
                "id": nsid,
                "defs": {"main": {"type": "permission-set", "permissions": [
                    {"type": "permission", "resource": "repo", "collection": ["com.example.cal.event"], "action": ["create"]},
                    {"type": "permission", "resource": "repo", "collection": ["app.bsky.feed.post"]},
                    {"type": "permission", "resource": "rpc", "inheritAud": true, "lxm": ["com.example.cal.listEvents"]},
                ]}},
            }}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    h.state
        .settings
        .set("oauth.lexicon_authority_did", json!(did), "test")
        .await
        .unwrap();
    let scope = crate::lexicon::token_scope(
        &h.state,
        "atproto include:com.example.cal.auth?aud=did:web:cal.example.com%23svc",
    )
    .await
    .unwrap();
    assert_eq!(
        scope,
        "repo:com.example.cal.event?action=create rpc:com.example.cal.listEvents?aud=did:web:cal.example.com%23svc atproto",
        "outside its namespace (app.bsky) is dropped"
    );
    assert!(
        crate::lexicon::token_scope(&h.state, "atproto include:com.example.cal.missing")
            .await
            .is_err()
    );
}

impl Harness {
    /// An OAuth grant for `did` to the loopback client `client_id`, through
    /// PAR, approval and the code exchange. Returns the token response.
    async fn oauth_grant(
        &self,
        client: &DpopClient,
        client_id: &str,
        scope: &str,
        did: &str,
    ) -> Value {
        let (verifier, challenge) = pkce();
        let redirect = "http://127.0.0.1/";
        let (st, _, v) = client
            .post(
                self,
                "/oauth/par",
                &[
                    ("client_id", client_id),
                    ("response_type", "code"),
                    ("scope", scope),
                    ("redirect_uri", redirect),
                    ("code_challenge", &challenge),
                    ("code_challenge_method", "S256"),
                ],
            )
            .await;
        assert_eq!(st, StatusCode::CREATED, "{v}");
        let id =
            locus_auth::oauth::request::request_id_from_uri(v["request_uri"].as_str().unwrap())
                .unwrap()
                .to_owned();
        crate::oauth::open_request(&self.state, &id, "dev-t", client_id)
            .await
            .unwrap();
        let (code, _) = crate::oauth::authorize(&self.state, &id, "dev-t", did, None)
            .await
            .unwrap();
        let (st, _, tok) = client
            .post(
                self,
                "/oauth/token",
                &[
                    ("grant_type", "authorization_code"),
                    ("code", &code),
                    ("redirect_uri", redirect),
                    ("code_verifier", &verifier),
                    ("client_id", client_id),
                ],
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{tok}");
        tok
    }

    /// An XRPC call with an OAuth access token and a fresh DPoP proof.
    async fn oauth_call(
        &self,
        client: &DpopClient,
        token: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, axum::http::HeaderMap, Value) {
        let url = format!("https://pds.example.com{}", path.split('?').next().unwrap());
        let proof = client.proof(method, &url, Some(token));
        let auth = format!("DPoP {token}");
        let mut hs: Vec<(&str, &str)> = vec![("authorization", &auth), ("dpop", &proof)];
        let body_bytes = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
        if !body_bytes.is_empty() {
            hs.push(("content-type", "application/json"));
        }
        let (st, headers, bytes) = self.raw(method, path, None, body_bytes, &hs).await;
        if let Some(n) = headers.get("dpop-nonce") {
            *client.nonce.lock().unwrap() = Some(n.to_str().unwrap().to_owned());
        }
        (
            st,
            headers,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

#[tokio::test]
async fn oauth_tokens_reach_endpoints_their_scopes_allow() {
    let h = harness(false).await;
    let acct = h
        .create_account("rsu.pds.example.com", "rsu@example.com", "pw")
        .await;
    let did = acct["did"].as_str().unwrap().to_owned();
    let client = DpopClient::new();
    let client_id = "http://localhost?scope=atproto+repo%3Aapp.bsky.feed.post%3Faction%3Dcreate";
    let tok = h
        .oauth_grant(
            &client,
            client_id,
            "atproto repo:app.bsky.feed.post?action=create",
            &did,
        )
        .await;
    let at = tok["access_token"].as_str().unwrap().to_owned();

    // getSession works, without the email (no account:email scope).
    let (st, headers, v) = h
        .oauth_call(
            &client,
            &at,
            "GET",
            "/xrpc/com.atproto.server.getSession",
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["did"], did.as_str());
    assert!(v.get("email").is_none() && v.get("emailConfirmed").is_none());
    assert!(
        headers.contains_key("dpop-nonce"),
        "a fresh nonce on every response"
    );

    // Creating a post is granted; deleting isn't.
    let record = json!({"repo": did, "collection": "app.bsky.feed.post", "rkey": "3m7aaaaaaaaad", "record": post_record("via oauth")});
    let (st, _, v) = h
        .oauth_call(
            &client,
            &at,
            "POST",
            "/xrpc/com.atproto.repo.createRecord",
            Some(record),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (st, _, v) = h
        .oauth_call(
            &client,
            &at,
            "POST",
            "/xrpc/com.atproto.repo.deleteRecord",
            Some(json!({"repo": did, "collection": "app.bsky.feed.post", "rkey": "3m7aaaaaaaaad"})),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str(), v["message"].as_str()),
        (
            StatusCode::FORBIDDEN,
            Some("ScopeMissingError"),
            Some("Missing required scope \"repo:app.bsky.feed.post?action=delete\"")
        )
    );
    let (st, _, v) = h
        .oauth_call(
            &client,
            &at,
            "POST",
            "/xrpc/com.atproto.server.createAppPassword",
            Some(json!({"name": "x"})),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str(), v["message"].as_str()),
        (
            StatusCode::FORBIDDEN,
            Some("Forbidden"),
            Some("OAuth credentials are not supported for this endpoint")
        )
    );
    let (st, _, v) = h
        .oauth_call(
            &client,
            &at,
            "POST",
            "/xrpc/com.atproto.server.activateAccount",
            None,
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("Forbidden"))
    );
    // Endpoints with no OAuth path refuse the scheme.
    let (st, _, v) = h
        .oauth_call(
            &client,
            &at,
            "POST",
            "/xrpc/com.atproto.server.updateEmail",
            Some(json!({"email": "x@example.com"})),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("InvalidToken"))
    );

    // Proof problems: none, a stale nonce, a replay.
    let auth = format!("DPoP {at}");
    let (st, _, b) = h
        .raw(
            "GET",
            "/xrpc/com.atproto.server.getSession",
            None,
            vec![],
            &[("authorization", &auth)],
        )
        .await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(
        (st, v["message"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("DPoP proof required"))
    );
    *client.nonce.lock().unwrap() = Some("stale".into());
    let proof = client.proof(
        "GET",
        "https://pds.example.com/xrpc/com.atproto.server.getSession",
        Some(&at),
    );
    let (st, headers, b) = h
        .raw(
            "GET",
            "/xrpc/com.atproto.server.getSession",
            None,
            vec![],
            &[("authorization", &auth), ("dpop", &proof)],
        )
        .await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("use_dpop_nonce"))
    );
    assert!(
        headers["www-authenticate"]
            .to_str()
            .unwrap()
            .starts_with("DPoP ")
    );
    assert!(
        headers["www-authenticate"]
            .to_str()
            .unwrap()
            .contains("error=\"use_dpop_nonce\"")
    );
    *client.nonce.lock().unwrap() = Some(headers["dpop-nonce"].to_str().unwrap().to_owned());
    let proof = client.proof(
        "GET",
        "https://pds.example.com/xrpc/com.atproto.server.getSession",
        Some(&at),
    );
    let hs = [("authorization", auth.as_str()), ("dpop", proof.as_str())];
    let (st, _, _) = h
        .raw(
            "GET",
            "/xrpc/com.atproto.server.getSession",
            None,
            vec![],
            &hs,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, b) = h
        .raw(
            "GET",
            "/xrpc/com.atproto.server.getSession",
            None,
            vec![],
            &hs,
        )
        .await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(
        (st, v["message"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("DPoP proof replayed"))
    );
    // Another key can't use the token.
    let thief = DpopClient::new();
    *thief.nonce.lock().unwrap() = client.nonce.lock().unwrap().clone();
    let (st, _, v) = h
        .oauth_call(
            &thief,
            &at,
            "GET",
            "/xrpc/com.atproto.server.getSession",
            None,
        )
        .await;
    assert_eq!(
        (st, v["message"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("Invalid DPoP key binding"))
    );

    // Revocation takes effect on the next request; so does a password reset.
    client
        .post(
            &h,
            "/oauth/revoke",
            &[
                ("token", tok["refresh_token"].as_str().unwrap()),
                ("client_id", client_id),
            ],
        )
        .await;
    let (st, _, v) = h
        .oauth_call(
            &client,
            &at,
            "GET",
            "/xrpc/com.atproto.server.getSession",
            None,
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("invalid_token"))
    );
    let tok = h
        .oauth_grant(
            &client,
            client_id,
            "atproto repo:app.bsky.feed.post?action=create",
            &did,
        )
        .await;
    let at = tok["access_token"].as_str().unwrap().to_owned();
    h.state
        .store
        .oauth_delete_account_tokens(&did)
        .await
        .unwrap();
    let (st, _, _) = h
        .oauth_call(
            &client,
            &at,
            "GET",
            "/xrpc/com.atproto.server.getSession",
            None,
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

/// A browser for the OAuth pages: cookies, the last page's CSRF token and
/// request id.
#[derive(Default)]
struct Browser {
    jar: Mutex<Vec<(String, String)>>,
}

impl Browser {
    fn cookie_header(&self) -> String {
        self.jar
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn keep(&self, headers: &axum::http::HeaderMap) {
        let mut jar = self.jar.lock().unwrap();
        for v in headers.get_all("set-cookie") {
            let c = v.to_str().unwrap();
            let (pair, attrs) = c.split_once(';').unwrap_or((c, ""));
            let (k, val) = pair.split_once('=').unwrap();
            jar.retain(|(n, _)| n != k);
            if !attrs.contains("Max-Age=0") {
                jar.push((k.to_owned(), val.to_owned()));
            }
        }
    }

    async fn get(&self, h: &Harness, path: &str) -> (StatusCode, axum::http::HeaderMap, String) {
        let cookie = self.cookie_header();
        let (st, headers, body) = h
            .raw("GET", path, None, vec![], &[("cookie", &cookie)])
            .await;
        self.keep(&headers);
        (st, headers, String::from_utf8_lossy(&body).into_owned())
    }

    async fn post(
        &self,
        h: &Harness,
        path: &str,
        form: &[(&str, &str)],
    ) -> (StatusCode, axum::http::HeaderMap, String) {
        let cookie = self.cookie_header();
        let body: String = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form.iter().copied())
            .finish();
        let (st, headers, body) = h
            .raw(
                "POST",
                path,
                None,
                body.into_bytes(),
                &[
                    ("cookie", &cookie),
                    ("content-type", "application/x-www-form-urlencoded"),
                    ("sec-fetch-site", "same-origin"),
                ],
            )
            .await;
        self.keep(&headers);
        (st, headers, String::from_utf8_lossy(&body).into_owned())
    }
}

/// A hidden field's value in a page.
fn hidden(html: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\" value=\"");
    let i = html
        .find(&marker)
        .unwrap_or_else(|| panic!("no {name} field in {html}"));
    let rest = &html[i + marker.len()..];
    rest[..rest.find('"').unwrap()].to_owned()
}

#[tokio::test]
async fn the_sign_in_pages_take_a_user_from_par_to_a_code() {
    const LB: &str = "http://localhost?scope=atproto+transition%3Ageneric";
    let h = harness(false).await;
    let acct = h
        .create_account("yuna.pds.example.com", "yuna@example.com", "hunter22")
        .await;
    let did = acct["did"].as_str().unwrap().to_owned();
    let client = DpopClient::new();
    let par = || async {
        let (verifier, challenge) = pkce();
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
                    ("state", "st8"),
                ],
            )
            .await;
        (v["request_uri"].as_str().unwrap().to_owned(), verifier)
    };
    let authorize_path = |uri: &str| {
        format!(
            "/oauth/authorize?client_id={}&request_uri={}",
            url::form_urlencoded::byte_serialize(LB.as_bytes()).collect::<String>(),
            url::form_urlencoded::byte_serialize(uri.as_bytes()).collect::<String>()
        )
    };

    // A new browser gets the sign-in page and its cookies.
    let browser = Browser::default();
    let (uri, verifier) = par().await;
    let (st, headers, html) = browser.get(&h, &authorize_path(&uri)).await;
    assert_eq!(st, StatusCode::OK);
    assert!(html.contains("Sign in to"), "{html}");
    let csp = headers["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("frame-ancestors 'none'") && csp.contains("script-src 'self'"));
    let jar = browser.cookie_header();
    assert!(
        jar.contains("dev-id=dev-") && jar.contains("ses-id=ses-") && jar.contains("csrf-token=")
    );
    let (csrf, request_id) = (hidden(&html, "csrf"), hidden(&html, "request_id"));

    // CSRF and password checks.
    let (st, _, _) = browser
        .post(
            &h,
            "/oauth/authorize/sign-in",
            &[
                ("csrf", "wrong"),
                ("request_id", &request_id),
                ("identifier", "yuna.pds.example.com"),
                ("password", "hunter22"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (_, _, html) = browser
        .post(
            &h,
            "/oauth/authorize/sign-in",
            &[
                ("csrf", &csrf),
                ("request_id", &request_id),
                ("identifier", "yuna.pds.example.com"),
                ("password", "nope"),
            ],
        )
        .await;
    assert!(html.contains("Invalid identifier or password"));

    // Sign in; a public client always gets the consent page.
    let (st, _, html) = browser
        .post(
            &h,
            "/oauth/authorize/sign-in",
            &[
                ("csrf", &csrf),
                ("request_id", &request_id),
                ("identifier", "yuna.pds.example.com"),
                ("password", "hunter22"),
                ("remember", "1"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        html.contains("wants access") && html.contains("Read and write your account data"),
        "{html}"
    );
    // Keep only atproto: the grant has only what was approved.
    let (st, headers, _) = browser
        .post(
            &h,
            "/oauth/authorize/accept",
            &[
                ("csrf", &csrf),
                ("request_id", &request_id),
                ("did", &did),
                ("scope", "atproto"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let location = url::Url::parse(headers["location"].to_str().unwrap()).unwrap();
    let q: HashMap<String, String> = location.query_pairs().into_owned().collect();
    assert_eq!(
        (location.host_str(), q["state"].as_str(), q["iss"].as_str()),
        (Some("127.0.0.1"), "st8", "https://pds.example.com")
    );
    let (st, _, tok) = client
        .post(
            &h,
            "/oauth/token",
            &[
                ("grant_type", "authorization_code"),
                ("code", &q["code"]),
                ("redirect_uri", "http://127.0.0.1/"),
                ("code_verifier", &verifier),
                ("client_id", LB),
            ],
        )
        .await;
    assert_eq!(
        (st, tok["scope"].as_str()),
        (StatusCode::OK, Some("atproto")),
        "{tok}"
    );

    // The same browser now sees the account in the picker; deny this time.
    let (uri, _) = par().await;
    let (_, _, html) = browser.get(&h, &authorize_path(&uri)).await;
    assert!(
        html.contains("@yuna.pds.example.com") && html.contains("Continue as"),
        "{html}"
    );
    let request_id = hidden(&html, "request_id");
    let csrf = hidden(&html, "csrf");
    let (_, _, html) = browser
        .post(
            &h,
            "/oauth/authorize/select",
            &[("csrf", &csrf), ("request_id", &request_id), ("did", &did)],
        )
        .await;
    assert!(html.contains("wants access"), "{html}");
    let (st, headers, _) = browser
        .post(
            &h,
            "/oauth/authorize/reject",
            &[("csrf", &csrf), ("request_id", &request_id)],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let loc = headers["location"].to_str().unwrap();
    assert!(
        loc.contains("error=access_denied") && loc.contains("state=st8"),
        "{loc}"
    );

    // Email 2FA on a device the account hasn't used.
    {
        let mut tx = h.state.store.begin().await.unwrap();
        tx.set_email_confirmed(&did).await.unwrap();
        tx.set_email_auth_factor(&did, "yuna@example.com", true)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let other = Browser::default();
    let (uri, _) = par().await;
    let (_, _, html) = other.get(&h, &authorize_path(&uri)).await;
    let (csrf, request_id) = (hidden(&html, "csrf"), hidden(&html, "request_id"));
    let creds = [
        ("csrf", csrf.as_str()),
        ("request_id", request_id.as_str()),
        ("identifier", "yuna@example.com"),
        ("password", "hunter22"),
    ];
    let (_, _, html) = other.post(&h, "/oauth/authorize/sign-in", &creds).await;
    assert!(html.contains("A sign-in code has been sent"), "{html}");
    let (_, code) = h.last_mail("yuna@example.com");
    let mut with_code = creds.to_vec();
    with_code.push(("code", &code));
    let (_, _, html) = other.post(&h, "/oauth/authorize/sign-in", &with_code).await;
    assert!(html.contains("wants access"), "{html}");
    // The first browser (seen in the last 30 days) skips the code.
    let (uri, _) = par().await;
    let (_, _, html) = browser.get(&h, &authorize_path(&uri)).await;
    let (csrf, request_id) = (hidden(&html, "csrf"), hidden(&html, "request_id"));
    let (_, _, html) = browser
        .post(
            &h,
            "/oauth/authorize/sign-in",
            &[
                ("csrf", &csrf),
                ("request_id", &request_id),
                ("identifier", "yuna.pds.example.com"),
                ("password", "hunter22"),
            ],
        )
        .await;
    assert!(html.contains("wants access"), "{html}");
}

// ---------------------------------------------------------------- kryphocron

const POST_PRIVATE: &str = "tools.kryphocron.feed.postPrivate";

impl Harness {
    /// An audience in `did`'s private audience store.
    async fn add_audience(&self, did: &str, rkey: &str) -> String {
        self.state
            .store
            .put_kr_audience(&locus_store::kryphocron::AudienceRow {
                did: did.into(),
                rkey: rkey.into(),
                name: None,
                mode: "list".into(),
                members_json: "[]".into(),
                created_at: "2024-01-01T00:00:00.000Z".into(),
                updated_at: "2024-01-01T00:00:00.000Z".into(),
                origin: "prism".into(),
                prism_did: "did:web:prism.test".into(),
                prism_cid: Some("3m6prismcid22".into()),
                local_cid: "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifvx5xytvske7mr3hpm".into(),
                value_cbor: vec![0xa0],
                pending: "none".into(),
                row_version: 1,
            })
            .await
            .unwrap();
        format!("at://{did}/tools.kryphocron.policy.audience/{rkey}")
    }

    /// A configured prism (the private AppView) that admits every caller to
    /// the private tier.
    async fn with_prism(&self) -> (Script, SeenLog) {
        let (url, script, seen) = scripted_appview().await;
        for (k, v) in [
            ("appview.private_url", url.as_str()),
            ("appview.private_did", "did:web:prism.test"),
        ] {
            self.state.settings.set(k, json!(v), "test").await.unwrap();
        }
        script.lock().unwrap().insert(
            "app.nearhorizon.actor.getMembership".into(),
            (
                200,
                vec![],
                json!({"status": "member", "features": [], "validForSecs": 3600,
                       "permissions": {"privateIndexed": true, "privateRead": true}}),
            ),
        );
        (script, seen)
    }
}

fn private_post(text: &str, audience: &str) -> Value {
    json!({"$type": POST_PRIVATE, "text": text, "createdAt": "2024-01-01T00:00:00.000Z", "audienceList": audience})
}

#[tokio::test]
async fn private_posts_are_gated_encoded_and_indexed() {
    let h = harness(false).await;
    let acct = h
        .create_account("kira.pds.example.com", "kira@example.com", "pw")
        .await;
    let (did, access) = (
        acct["did"].as_str().unwrap().to_owned(),
        acct["accessJwt"].as_str().unwrap().to_owned(),
    );
    let audience = h.add_audience(&did, "3m6aaaaaaaaaa").await;
    let create = |record: Value| {
        let (h, access, did) = (&h, access.clone(), did.clone());
        async move {
            h.post(
                "tools.kryphocron.feed.createPostPrivate",
                Some(&access),
                json!({"repo": did, "record": record}),
            )
            .await
        }
    };

    // No AppView: the private tier is off.
    let (st, v) = create(private_post("hi", &audience)).await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("UnsupportedNamespace")),
        "{v}"
    );

    let (script, seen) = h.with_prism().await;
    let mention = json!([{"index": {"byteStart": 0, "byteEnd": 5},
        "features": [{"$type": "app.bsky.richtext.facet#mention", "did": "did:plc:friendfriendfriend"}]}]);
    let mut rec = private_post("@kira the private sentinel", &audience);
    rec["facets"] = mention;
    rec["mentionDids"] = json!(["did:plc:claimedbutfalse"]);
    let (st, v) = create(rec).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let uri = v["uri"].as_str().unwrap().to_owned();
    let rkey = uri.rsplit('/').next().unwrap().to_owned();

    // Stored encoded: no text, derived clear fields, a generation mark.
    let (st, got) = h
        .get(format!(
            "/xrpc/com.atproto.repo.getRecord?repo={did}&collection={POST_PRIVATE}&rkey={rkey}"
        ))
        .await;
    assert_eq!(st, StatusCode::OK);
    let value = &got["value"];
    assert!(
        value.get("text").is_none() && value.get("facets").is_none(),
        "{value}"
    );
    assert_eq!(value["encodedContentCodec"], "laquna/0.2");
    assert!(
        value["encodedContentGeneration"]
            .as_str()
            .unwrap()
            .starts_with("laquna/")
    );
    assert_eq!(value["mentionDids"], json!(["did:plc:friendfriendfriend"]));
    assert!(value["encodedContent"]["$bytes"].is_string());
    let block = h
        .state
        .store
        .block(&did, got["cid"].as_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(
        !block.windows(8).any(|w| w == b"sentinel"),
        "no plaintext in the repo"
    );
    for (_, event) in h.state.store.outbox_for(&did).await.unwrap() {
        assert!(
            !event.windows(8).any(|w| w == b"sentinel"),
            "nor on the firehose"
        );
    }

    // Indexed for rotation, with its audience.
    let entry = h
        .state
        .store
        .private_record(&did, POST_PRIVATE, &rkey)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        entry.generation.as_deref(),
        value["encodedContentGeneration"].as_str()
    );
    assert_eq!(
        (entry.audience_rkey.as_deref(), entry.legacy_text),
        (Some("3m6aaaaaaaaaa"), false)
    );

    // The generic endpoints run the same pipeline.
    let (st, v) = h
        .post(
            "com.atproto.repo.createRecord",
            Some(&access),
            json!({"repo": did, "collection": POST_PRIVATE, "record": private_post("generic", &audience)}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let generic_rkey = v["uri"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    let (_, got) = h
        .get(format!("/xrpc/com.atproto.repo.getRecord?repo={did}&collection={POST_PRIVATE}&rkey={generic_rkey}"))
        .await;
    assert!(got["value"].get("text").is_none() && got["value"]["encodedContent"].is_object());

    // getMembership was asked once; its answer is cached.
    let asked = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|s| s.uri.contains("getMembership"))
        .count();
    assert_eq!(asked, 1);

    // Host rules. Prism's complete listing has only the one audience.
    set_script(
        &script,
        "app.nearhorizon.private.listAudiences",
        200,
        json!({"audiences": [prism_audience(&did, "3m6aaaaaaaaaa", "3m6prismcid22", &[])]}),
    );
    let (st, v) = create(private_post(
        "x",
        &format!("at://{did}/tools.kryphocron.policy.audience/3m6zzzzzzzzzz"),
    ))
    .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "an audience the store doesn't hold: {v}"
    );
    let (st, _) = create(private_post(
        "x",
        "at://did:plc:someoneelse/tools.kryphocron.policy.audience/3m6aaaaaaaaaa",
    ))
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "someone else's audience");
    let mut both = private_post("x", &audience);
    both["encodedContent"] = json!({"$bytes": "AAAA"});
    both["encodedContentCodec"] = json!("laquna/0.2");
    assert_eq!(
        create(both).await.0,
        StatusCode::BAD_REQUEST,
        "text and encodedContent"
    );
    let (st, v) = h
        .post(
            "com.atproto.repo.createRecord",
            Some(&access),
            json!({"repo": did, "collection": "tools.kryphocron.feed.nothing", "record": {"createdAt": "2024-01-01T00:00:00.000Z"}}),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "unregistered: {v}");
    let (st, _) = h
        .post(
            "com.atproto.repo.putRecord",
            Some(&access),
            json!({"repo": did, "collection": "tools.kryphocron.graph.block", "rkey": "self",
                   "record": {"subject": "did:plc:someoneelse", "createdAt": "2024-01-01T00:00:00.000Z"}}),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "kryphocron keys are TIDs");

    // updatePostPrivate changes only the audience.
    let other = h.add_audience(&did, "3m6bbbbbbbbbb").await;
    let (st, v) = h
        .post(
            "tools.kryphocron.feed.updatePostPrivate",
            Some(&access),
            json!({"repo": did, "rkey": rkey, "audienceList": other, "swapRecord": "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifvx5xytvske7mr3hpm"}),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("InvalidSwap"))
    );
    let (st, v) = h
        .post(
            "tools.kryphocron.feed.updatePostPrivate",
            Some(&access),
            json!({"repo": did, "rkey": rkey, "audienceList": other, "swapRecord": got_cid(&h, &did, &rkey).await}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (_, after) = h
        .get(format!(
            "/xrpc/com.atproto.repo.getRecord?repo={did}&collection={POST_PRIVATE}&rkey={rkey}"
        ))
        .await;
    assert_eq!(
        after["value"]["encodedContent"], value["encodedContent"],
        "content kept byte for byte"
    );
    assert_eq!(after["value"]["audienceList"], json!(other));
    assert_ne!(after["cid"], json!(got_cid_value(&got)));

    // The local block, and the host switch.
    h.state
        .store
        .set_private_tier_blocked(&did, true)
        .await
        .unwrap();
    assert_eq!(
        create(private_post("x", &audience)).await.0,
        StatusCode::FORBIDDEN
    );
    h.state
        .store
        .set_private_tier_blocked(&did, false)
        .await
        .unwrap();
    h.state
        .settings
        .set("kryphocron.private_tier_enabled", json!(false), "test")
        .await
        .unwrap();
    let (_, v) = create(private_post("x", &audience)).await;
    assert_eq!(v["error"], "UnsupportedNamespace");

    // Deletes are never gated.
    let (st, _) = h
        .post(
            "tools.kryphocron.feed.deletePostPrivate",
            Some(&access),
            json!({"repo": did, "rkey": rkey}),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        h.state
            .store
            .private_record(&did, POST_PRIVATE, &rkey)
            .await
            .unwrap()
            .is_none(),
        "unindexed"
    );
    let (st, _) = h
        .post(
            "com.atproto.repo.deleteRecord",
            Some(&access),
            json!({"repo": did, "collection": POST_PRIVATE, "rkey": generic_rkey}),
        )
        .await;
    assert_eq!(st, StatusCode::OK);

    // Audit: the encodes were recorded.
    // The audit writer runs in the background: wait for it, briefly.
    let mut audit = Vec::new();
    for _ in 0..50 {
        audit = h.state.store.kr_audit(50).await.unwrap();
        if audit.iter().any(|r| r.event == "ContentEncoded") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        audit.iter().any(|r| r.event == "ContentEncoded"),
        "{audit:?}"
    );
}

async fn got_cid(h: &Harness, did: &str, rkey: &str) -> String {
    h.state
        .store
        .record_entry(did, POST_PRIVATE, rkey)
        .await
        .unwrap()
        .unwrap()
        .0
}

fn got_cid_value(got: &Value) -> String {
    got["cid"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn the_appview_decides_who_may_write_private_posts() {
    let h = harness(false).await;
    let acct = h
        .create_account("noor.pds.example.com", "noor@example.com", "pw")
        .await;
    let (did, access) = (
        acct["did"].as_str().unwrap().to_owned(),
        acct["accessJwt"].as_str().unwrap().to_owned(),
    );
    let audience = h.add_audience(&did, "3m6aaaaaaaaaa").await;
    let (script, _) = h.with_prism().await;
    let set = |status: u16, body: Value| {
        script.lock().unwrap().insert(
            "app.nearhorizon.actor.getMembership".into(),
            (status, vec![], body),
        );
    };
    let create = || {
        let (h, access, did, audience) = (&h, access.clone(), did.clone(), audience.clone());
        async move {
            h.post(
                "tools.kryphocron.feed.createPostPrivate",
                Some(&access),
                json!({"repo": did, "record": private_post("x", &audience)}),
            )
            .await
        }
    };
    set(
        200,
        json!({"status": "notEligible", "validForSecs": 5, "permissions": {"privateIndexed": false, "privateRead": false}}),
    );
    assert_eq!(create().await.0, StatusCode::FORBIDDEN);
    // The refusal is cached for its validForSecs; let it lapse.
    let mut row = h
        .state
        .store
        .kr_permission(&did, "membership")
        .await
        .unwrap()
        .unwrap();
    row.expires_at = 0;
    h.state.store.put_kr_permission(&row).await.unwrap();
    set(503, json!({"error": "Unavailable"}));
    let (st, v) = create().await;
    assert_eq!(
        st,
        StatusCode::SERVICE_UNAVAILABLE,
        "no answer is retryable: {v}"
    );
    set(400, json!({"error": "AccountTakedown"}));
    assert_eq!(create().await.0, StatusCode::FORBIDDEN);
    let probe = h
        .state
        .store
        .kr_permission(&did, "probe")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(probe.status, "takedown", "a takedown writes both rows");
    // Indexing members may write (privateIndexed), once the takedown lapses.
    for source in ["membership", "probe"] {
        let mut row = h
            .state
            .store
            .kr_permission(&did, source)
            .await
            .unwrap()
            .unwrap();
        row.expires_at = 0;
        h.state.store.put_kr_permission(&row).await.unwrap();
    }
    set(
        200,
        json!({"status": "indexing", "validForSecs": 5, "permissions": {"privateIndexed": true, "privateRead": false}}),
    );
    assert_eq!(create().await.0, StatusCode::OK);

    // An unreachable prism: the last answer stands, expired or not.
    let mut row = h
        .state
        .store
        .kr_permission(&did, "membership")
        .await
        .unwrap()
        .unwrap();
    row.expires_at = 0;
    h.state.store.put_kr_permission(&row).await.unwrap();
    let dead = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    h.state
        .settings
        .set("appview.private_url", json!(dead), "test")
        .await
        .unwrap();
    assert_eq!(
        create().await.0,
        StatusCode::OK,
        "outage: the last answer stands"
    );
    h.state.store.delete_kr_permissions(&did).await.unwrap();
    assert_eq!(
        create().await.0,
        StatusCode::FORBIDDEN,
        "outage with no answer at all"
    );
}

impl Harness {
    /// Commits a record as stored, skipping kryphocron's step (as an import
    /// would bring it).
    async fn commit_raw(&self, did: &str, collection: &str, rkey: &str, value: Value) {
        let account = self.state.store.account_by_did(did).await.unwrap().unwrap();
        let rec = locus_repo::records::prepare_record(
            collection,
            rkey,
            locus_repo::records::parse_json(&value).unwrap(),
            Some(false),
            usize::MAX,
        )
        .unwrap();
        let mut w = crate::repo::prepared(
            locus_repo::Action::Create,
            collection,
            rkey,
            Some(&rec),
            None,
        );
        w.lenient_blobs = true;
        crate::repo::commit(&self.state, &account, vec![w], None, false)
            .await
            .unwrap();
    }

    async fn record_value(&self, did: &str, collection: &str, rkey: &str) -> Value {
        let (st, v) = self
            .get(format!(
                "/xrpc/com.atproto.repo.getRecord?repo={did}&collection={collection}&rkey={rkey}"
            ))
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        v["value"].clone()
    }
}

#[tokio::test]
async fn rotation_re_encodes_private_posts_under_the_new_generation() {
    let h = harness(false).await;
    let acct = h
        .create_account("ines.pds.example.com", "ines@example.com", "pw")
        .await;
    let (did, access) = (
        acct["did"].as_str().unwrap().to_owned(),
        acct["accessJwt"].as_str().unwrap().to_owned(),
    );
    let audience = h.add_audience(&did, "3m6aaaaaaaaaa").await;
    h.with_prism().await;
    let mut rkeys = Vec::new();
    for text in ["first", "second"] {
        let (st, v) = h
            .post(
                "tools.kryphocron.feed.createPostPrivate",
                Some(&access),
                json!({"repo": did, "record": private_post(text, &audience)}),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        rkeys.push(
            v["uri"]
                .as_str()
                .unwrap()
                .rsplit('/')
                .next()
                .unwrap()
                .to_owned(),
        );
    }
    let before = h.record_value(&did, POST_PRIVATE, &rkeys[0]).await;

    // Imported plaintext, its image not uploaded yet; and an undecodable one.
    let missing = proto_blue_lex_data::Cid::for_raw(b"not uploaded yet").to_string_base32();
    let mut legacy = private_post("an imported plaintext post", &audience);
    legacy["embed"] = json!({"$type": "app.bsky.embed.images", "images": [{"alt": "",
        "image": {"$type": "blob", "ref": {"$link": missing}, "mimeType": "image/png", "size": 9}}]});
    h.commit_raw(&did, POST_PRIVATE, "3m6legacylega", legacy)
        .await;
    let garbage = json!({"$type": POST_PRIVATE, "createdAt": "2024-01-01T00:00:00.000Z", "audienceList": audience,
        "encodedContent": {"$bytes": "AAECAwQFBgc"}, "encodedContentCodec": "laquna/0.2"});
    h.commit_raw(&did, POST_PRIVATE, "3m6garbagegar", garbage)
        .await;
    assert!(
        h.state
            .store
            .private_record(&did, POST_PRIVATE, "3m6legacylega")
            .await
            .unwrap()
            .unwrap()
            .legacy_text
    );

    // A new generation, made just under a period ago, so every account is
    // past its spread offset.
    let now = locus_core::time::now_ms();
    let next = locus_kryphocron::rotation::new_mark(u64::try_from(now / 1000).unwrap() + 1);
    h.state
        .store
        .add_generation(&next, now - 60 * 86_400_000 + 60_000)
        .await
        .unwrap();
    let lease = h
        .state
        .store
        .try_acquire_lease(
            "kr-rotation",
            "test-node",
            std::time::Duration::from_secs(60),
        )
        .await
        .unwrap()
        .unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    h.state
        .settings
        .set(
            "kryphocron.rotation_max_commits_per_minute",
            json!(10_000),
            "test",
        )
        .await
        .unwrap();
    let report = crate::kryphocron::rotation_pass(&h.state, &lease, &cancel)
        .await
        .unwrap();
    assert_eq!((report.rewritten, report.flagged), (3, 1), "{report:?}");
    assert_eq!(
        h.state.store.current_generation().await.unwrap().unwrap().0,
        next,
        "no extra generation"
    );

    // Encoded posts: new content and mark, everything else as it was.
    let after = h.record_value(&did, POST_PRIVATE, &rkeys[0]).await;
    assert_eq!(after["encodedContentGeneration"], json!(next));
    assert_ne!(after["encodedContent"], before["encodedContent"]);
    for k in ["createdAt", "audienceList", "encodedContentCodec", "$type"] {
        assert_eq!(after[k], before[k], "{k}");
    }
    let decode = |v: &Value, rkey: &str| {
        let lex = proto_blue_lex_json::json_to_lex_with(
            v,
            proto_blue_lex_json::LexParseOptions::strict(),
        )
        .unwrap();
        let bytes = lex.as_map().unwrap()["encodedContent"]
            .as_bytes()
            .unwrap()
            .to_vec();
        let (h, rkey, did) = (&h, rkey.to_owned(), did.clone());
        async move {
            let plain = h
                .state
                .kr
                .encoder
                .trial_decode(&did, POST_PRIVATE, &rkey, bytes)
                .await
                .unwrap();
            locus_kryphocron::kryphocron::post_content::PostContent::from_payload(&plain).unwrap()
        }
    };
    assert_eq!(decode(&after, &rkeys[0]).await.text, "first");

    // The imported plaintext is encoded now, its blob reference kept.
    let legacy = h.record_value(&did, POST_PRIVATE, "3m6legacylega").await;
    assert!(
        legacy.get("text").is_none() && legacy.get("embed").is_none(),
        "{legacy}"
    );
    assert_eq!(legacy["mediaKinds"], json!(["image"]));
    assert_eq!(legacy["embedBlobs"][0]["ref"]["$link"], json!(missing));
    let content = decode(&legacy, "3m6legacylega").await;
    assert_eq!(content.text, "an imported plaintext post");
    assert!(content.embed.is_some());
    let entry = h
        .state
        .store
        .private_record(&did, POST_PRIVATE, "3m6legacylega")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (entry.legacy_text, entry.generation.as_deref()),
        (false, Some(next.as_str()))
    );

    // The undecodable one is flagged and left alone.
    let flagged = h.state.store.flagged_private_records(10).await.unwrap();
    assert_eq!(
        flagged.iter().map(|r| r.rkey.as_str()).collect::<Vec<_>>(),
        ["3m6garbagegar"]
    );

    // Nothing left to do.
    let again = crate::kryphocron::rotation_pass(&h.state, &lease, &cancel)
        .await
        .unwrap();
    assert_eq!(again.rewritten, 0);
}

/// One `listAudiences` entry as prism answers.
fn prism_audience(did: &str, rkey: &str, cid: &str, members: &[&str]) -> Value {
    json!({
        "uri": format!("at://{did}/tools.kryphocron.policy.audience/{rkey}"),
        "cid": cid,
        "record": {"$type": "tools.kryphocron.policy.audience", "members": members,
                   "createdAt": "2024-01-01T00:00:00.000Z", "updatedAt": "2024-01-01T00:00:00.000Z"}
    })
}

fn set_script(script: &Script, key: &str, status: u16, body: Value) {
    script
        .lock()
        .unwrap()
        .insert(key.into(), (status, vec![], body));
}

#[tokio::test]
async fn a_post_naming_an_audience_the_copy_lacks_refreshes_it_from_prism() {
    let h = harness(false).await;
    let (script, seen) = h.with_prism().await;
    let account = |handle: &'static str| {
        let h = &h;
        async move {
            let a = h
                .create_account(handle, &format!("{handle}@x.com"), "pw")
                .await;
            (
                a["did"].as_str().unwrap().to_owned(),
                a["accessJwt"].as_str().unwrap().to_owned(),
            )
        }
    };
    let post = |did: String, access: String, rkey: &'static str| {
        let h = &h;
        async move {
            h.post(
                "tools.kryphocron.feed.createPostPrivate",
                Some(&access),
                json!({"repo": did, "record": private_post("x", &format!("at://{did}/tools.kryphocron.policy.audience/{rkey}"))}),
            )
            .await
        }
    };

    // The audience is on the second page of the owner's listing.
    let (did, access) = account("ana.pds.example.com").await;
    set_script(
        &script,
        "app.nearhorizon.private.listAudiences",
        200,
        json!({"cursor": "3m6bbbbbbbbbb", "audiences": [prism_audience(&did, "3m6cccccccccc", "v1", &[])]}),
    );
    set_script(
        &script,
        "app.nearhorizon.private.listAudiences#3m6bbbbbbbbbb",
        200,
        json!({"audiences": [prism_audience(&did, "3m6aaaaaaaaaa", "v1", &["did:plc:friend"])]}),
    );
    let (st, v) = post(did.clone(), access.clone(), "3m6aaaaaaaaaa").await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let row = h
        .state
        .store
        .kr_audience(&did, "3m6aaaaaaaaaa")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            row.origin.as_str(),
            row.members_json.as_str(),
            row.prism_cid.as_deref()
        ),
        ("prism", "[\"did:plc:friend\"]", Some("v1"))
    );
    assert!(
        h.state
            .store
            .kr_audience(&did, "3m6cccccccccc")
            .await
            .unwrap()
            .is_some(),
        "the first page is adopted too"
    );
    let calls = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|s| s.uri.contains("listAudiences"))
        .count();
    assert_eq!(calls, 2, "stopped at the page holding the URI");

    // Missing from a complete listing: refused, not retried.
    let (did2, access2) = account("bea.pds.example.com").await;
    set_script(
        &script,
        "app.nearhorizon.private.listAudiences",
        200,
        json!({"audiences": []}),
    );
    let (st, v) = post(did2.clone(), access2.clone(), "3m6aaaaaaaaaa").await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("InvalidRequest")),
        "{v}"
    );
    // The owner's own refreshes are limited: the next miss within 5 s is
    // retryable rather than another call.
    let (st, _) = post(did2.clone(), access2.clone(), "3m6aaaaaaaaaa").await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);

    // A failed listing is retryable.
    let (did3, access3) = account("cia.pds.example.com").await;
    set_script(
        &script,
        "app.nearhorizon.private.listAudiences",
        429,
        json!({"error": "RateLimitExceeded"}),
    );
    let (st, _) = post(did3.clone(), access3, "3m6aaaaaaaaaa").await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);

    // A complete listing deletes what prism no longer has, and a tombstone
    // keeps a stale page from bringing it back.
    set_script(
        &script,
        "app.nearhorizon.private.listAudiences",
        200,
        json!({"audiences": [prism_audience(&did, "3m6aaaaaaaaaa", "v1", &["did:plc:friend"])]}),
    );
    let far = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let r = crate::audience::refresh(
        &h.state,
        &did,
        crate::audience::Trigger::Background,
        None,
        far,
    )
    .await
    .unwrap();
    assert!(r.complete && r.error.is_none(), "{r:?}");
    assert!(
        h.state
            .store
            .kr_audience(&did, "3m6cccccccccc")
            .await
            .unwrap()
            .is_none(),
        "deleted in prism"
    );
    assert!(
        h.state
            .store
            .audience_tombstone(&did, "3m6cccccccccc")
            .await
            .unwrap()
            .is_some()
    );
    set_script(
        &script,
        "app.nearhorizon.private.listAudiences",
        200,
        json!({"audiences": [
            prism_audience(&did, "3m6aaaaaaaaaa", "v2", &["did:plc:friend", "did:plc:other"]),
            prism_audience(&did, "3m6cccccccccc", "v1", &[]),
        ]}),
    );
    crate::audience::refresh(
        &h.state,
        &did,
        crate::audience::Trigger::Background,
        None,
        far,
    )
    .await
    .unwrap();
    assert!(
        h.state
            .store
            .kr_audience(&did, "3m6cccccccccc")
            .await
            .unwrap()
            .is_none(),
        "the tombstone wins"
    );
    let row = h
        .state
        .store
        .kr_audience(&did, "3m6aaaaaaaaaa")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.prism_cid.as_deref(), row.row_version),
        (Some("v2"), 2),
        "a changed version token replaces the copy"
    );

    // A mapped local URI is stored as the prism URI it maps to.
    sqlx_alias(&h, &did, "3m6llllllllll", "3m6aaaaaaaaaa").await;
    let (st, v) = post(did.clone(), access.clone(), "3m6llllllllll").await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let rkey = v["uri"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(
        h.record_value(&did, POST_PRIVATE, &rkey).await["audienceList"],
        json!(format!(
            "at://{did}/tools.kryphocron.policy.audience/3m6aaaaaaaaaa"
        ))
    );

    // NotEnrolled: the copy and both permission rows are purged.
    set_script(
        &script,
        "app.nearhorizon.private.listAudiences",
        400,
        json!({"error": "NotEnrolled"}),
    );
    crate::audience::refresh(
        &h.state,
        &did,
        crate::audience::Trigger::Background,
        None,
        far,
    )
    .await
    .unwrap();
    assert!(h.state.store.kr_audiences(&did).await.unwrap().is_empty());
    assert!(
        h.state
            .store
            .kr_permission(&did, "membership")
            .await
            .unwrap()
            .is_none()
    );
}

/// Inserts an alias row (the mapping transaction comes with the sync job).
async fn sqlx_alias(h: &Harness, did: &str, local: &str, prism: &str) {
    h.state
        .store
        .put_audience_alias(did, local, prism)
        .await
        .unwrap();
}

#[tokio::test]
async fn an_unreachable_prism_is_marked_down_and_probed_back_up() {
    let h = harness(false).await;
    let acct = h
        .create_account("dee.pds.example.com", "dee@example.com", "pw")
        .await;
    let did = acct["did"].as_str().unwrap().to_owned();
    let (_script, _) = h.with_prism().await;
    let dead = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let live = h.state.snapshot().text("appview.private_url").to_owned();
    h.state
        .settings
        .set("appview.private_url", json!(dead), "test")
        .await
        .unwrap();
    let answer = crate::audience::call(
        &h.state,
        &did,
        "app.nearhorizon.private.listAudiences",
        &[],
        None,
    )
    .await;
    assert!(matches!(answer, crate::audience::Prism::Unreachable));
    // The probe (also unreachable) marks it down.
    for _ in 0..50 {
        if h.state.kr.prism.is_down() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(h.state.kr.prism.is_down());
    let far = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let r = crate::audience::refresh(
        &h.state,
        &did,
        crate::audience::Trigger::Background,
        None,
        far,
    )
    .await
    .unwrap();
    assert_eq!(
        r.error.as_deref(),
        Some("prism down"),
        "no calls while down"
    );
    // Any HTTP answer brings it back (the mock answers did.json with 404).
    h.state
        .settings
        .set("appview.private_url", json!(live), "test")
        .await
        .unwrap();
    for _ in 0..700 {
        if !h.state.kr.prism.is_down() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(!h.state.kr.prism.is_down(), "probed back up");
}

const AUD: &str = "tools.kryphocron.policy.audience";

#[tokio::test]
async fn audiences_are_written_through_to_prism_and_kept_private() {
    let h = harness(false).await;
    let acct = h
        .create_account("eve.pds.example.com", "eve@example.com", "pw")
        .await;
    let (did, access) = (
        acct["did"].as_str().unwrap().to_owned(),
        acct["accessJwt"].as_str().unwrap().to_owned(),
    );
    let (script, seen) = h.with_prism().await;
    let prism_uri = format!("at://{did}/{AUD}/3m6prismaaaaa");
    set_script(
        &script,
        "app.nearhorizon.private.createAudience",
        200,
        json!({"uri": prism_uri, "cid": "t1"}),
    );
    let outbox_before = h.state.store.outbox_for(&did).await.unwrap().len();

    let (st, v) = h
        .post(
            "tools.kryphocron.policy.manageAudience",
            Some(&access),
            json!({"repo": did, "record": {"name": "  Friends ", "members": ["did:plc:friend", "did:plc:friend", did], "extra": 1}}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["uri"], json!(prism_uri));
    let row = h
        .state
        .store
        .kr_audience(&did, "3m6prismaaaaa")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["cid"], json!(row.local_cid), "the stored value's CID");
    assert_eq!(
        (
            row.name.as_deref(),
            row.members_json.as_str(),
            row.prism_cid.as_deref()
        ),
        (Some("Friends"), "[\"did:plc:friend\"]", Some("t1"))
    );
    let sent: Value = {
        let log = seen.lock().unwrap();
        let s = log
            .iter()
            .find(|s| s.uri.contains("createAudience"))
            .unwrap();
        serde_json::from_slice(&s.body).unwrap()
    };
    assert_eq!(
        sent["record"]["createdAt"], sent["record"]["updatedAt"],
        "one instant, stamped by locus"
    );
    assert!(
        sent["record"].get("extra").is_none() && sent["record"].get("mode").is_none(),
        "prism's canonical form"
    );
    assert_eq!(
        h.state.store.outbox_for(&did).await.unwrap().len(),
        outbox_before,
        "no commit, no firehose event"
    );

    // Only the owner reads it.
    let path =
        format!("/xrpc/com.atproto.repo.getRecord?repo={did}&collection={AUD}&rkey=3m6prismaaaaa");
    let (st, mine) = h.call("GET", &path, Some(&access), None).await;
    assert_eq!(st, StatusCode::OK, "{mine}");
    assert_eq!(mine["value"]["members"], json!(["did:plc:friend"]));
    let (st, theirs) = h.call("GET", &path, None, None).await;
    assert_eq!(
        (st, theirs["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("RecordNotFound"))
    );
    let list = format!("/xrpc/com.atproto.repo.listRecords?repo={did}&collection={AUD}");
    assert_eq!(
        h.call("GET", &list, Some(&access), None).await.1["records"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        h.call("GET", &list, None, None).await.1["records"],
        json!([])
    );

    // Updates through putRecord, CAS on the stored value's CID.
    set_script(
        &script,
        "app.nearhorizon.private.updateAudience",
        200,
        json!({"uri": prism_uri, "cid": "t2"}),
    );
    let put = |swap: Value| {
        let (h, access, did) = (&h, access.clone(), did.clone());
        async move {
            h.post(
                "com.atproto.repo.putRecord",
                Some(&access),
                json!({"repo": did, "collection": AUD, "rkey": "3m6prismaaaaa", "swapRecord": swap,
                       "record": {"members": ["did:plc:friend", "did:plc:new"]}}),
            )
            .await
        }
    };
    let (st, v) = put(json!(
        "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifvx5xytvske7mr3hpm"
    ))
    .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("InvalidSwap"))
    );
    let (st, v) = put(json!(row.local_cid)).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let row2 = h
        .state
        .store
        .kr_audience(&did, "3m6prismaaaaa")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row2.prism_cid.as_deref(), row2.created_at.as_str()),
        (Some("t2"), row.created_at.as_str()),
        "createdAt kept"
    );
    let sent: Value = {
        let log = seen.lock().unwrap();
        serde_json::from_slice(
            &log.iter()
                .rev()
                .find(|s| s.uri.contains("updateAudience"))
                .unwrap()
                .body,
        )
        .unwrap()
    };
    assert_eq!(
        sent["swapCid"],
        json!("t1"),
        "prism's version token for its own CAS"
    );

    // createRecord with an rkey the listing lacks; applyWrites alone or not at all.
    set_script(
        &script,
        "app.nearhorizon.private.listAudiences",
        200,
        json!({"audiences": [prism_audience(&did, "3m6prismaaaaa", "t2", &["did:plc:friend", "did:plc:new"])]}),
    );
    let (st, _) = h
        .post("com.atproto.repo.createRecord", Some(&access),
            json!({"repo": did, "collection": AUD, "rkey": "3m6nopenopeno", "record": {"members": []}}))
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "prism mints audience rkeys");
    let (st, _) = h
        .post("com.atproto.repo.applyWrites", Some(&access), json!({"repo": did, "writes": [
            {"$type": "com.atproto.repo.applyWrites#delete", "collection": AUD, "rkey": "3m6prismaaaaa"},
            {"$type": "com.atproto.repo.applyWrites#create", "collection": "app.bsky.feed.post", "value": post_record("x")},
        ]}))
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "an audience op stands alone");

    // Prism's refusals, and locus's own cap.
    set_script(
        &script,
        "app.nearhorizon.private.createAudience",
        400,
        json!({"error": "LimitReached", "message": "200 max"}),
    );
    let create = || {
        let (h, access, did) = (&h, access.clone(), did.clone());
        async move {
            h.post(
                "tools.kryphocron.policy.manageAudience",
                Some(&access),
                json!({"repo": did, "record": {"members": []}}),
            )
            .await
        }
    };
    let (st, v) = create().await;
    assert_eq!(
        (st, v["message"].as_str()),
        (StatusCode::BAD_REQUEST, Some("200 max"))
    );
    h.state
        .settings
        .set("kryphocron.audience_max_per_account", json!(1), "test")
        .await
        .unwrap();
    let (_, v) = create().await;
    assert_eq!(v["error"], "LimitReached", "checked before calling prism");
    h.state
        .settings
        .set("kryphocron.audience_max_per_account", json!(200), "test")
        .await
        .unwrap();

    // Deletes, through applyWrites alone: no commit in the output.
    set_script(
        &script,
        "app.nearhorizon.private.deleteAudience",
        200,
        json!({}),
    );
    let (st, v) = h
        .post("com.atproto.repo.applyWrites", Some(&access), json!({"repo": did, "writes": [
            {"$type": "com.atproto.repo.applyWrites#delete", "collection": AUD, "rkey": "3m6prismaaaaa"}]}))
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(v.get("commit").is_none());
    assert!(
        h.state
            .store
            .kr_audience(&did, "3m6prismaaaaa")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        h.state
            .store
            .audience_tombstone(&did, "3m6prismaaaaa")
            .await
            .unwrap()
            .is_some()
    );

    // NotEnrolled: no longer a member; the copy goes.
    h.add_audience(&did, "3m6bbbbbbbbbb").await;
    set_script(
        &script,
        "app.nearhorizon.private.createAudience",
        400,
        json!({"error": "NotEnrolled"}),
    );
    assert_eq!(create().await.0, StatusCode::FORBIDDEN);
    assert!(h.state.store.kr_audiences(&did).await.unwrap().is_empty());
}

#[tokio::test]
async fn during_a_prism_outage_the_copy_is_the_authority() {
    let h = harness(false).await;
    let acct = h
        .create_account("fay.pds.example.com", "fay@example.com", "pw")
        .await;
    let (did, access) = (
        acct["did"].as_str().unwrap().to_owned(),
        acct["accessJwt"].as_str().unwrap().to_owned(),
    );
    let (_script, _) = h.with_prism().await;
    let existing = h.add_audience(&did, "3m6aaaaaaaaaa").await;
    // A first private-tier write while prism is up, so a membership answer
    // exists to fall back on.
    let (st, v) = h
        .post(
            "tools.kryphocron.feed.createPostPrivate",
            Some(&access),
            json!({"repo": did, "record": private_post("x", &existing)}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");

    let dead = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    h.state
        .settings
        .set("appview.private_url", json!(dead), "test")
        .await
        .unwrap();

    // Creating makes a local audience with a locus-minted TID.
    let (st, v) = h
        .post(
            "tools.kryphocron.policy.manageAudience",
            Some(&access),
            json!({"repo": did, "record": {"members": ["did:plc:friend"]}}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let local_rkey = v["uri"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    let row = h
        .state
        .store
        .kr_audience(&did, &local_rkey)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.origin.as_str(), row.prism_cid.as_deref()),
        ("local", None)
    );
    assert!(proto_blue_syntax::Tid::new(&local_rkey).is_ok());
    for _ in 0..50 {
        if h.state.kr.prism.is_down() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(h.state.kr.prism.is_down());

    // Posts can name it; editing it changes its row directly.
    let (st, v) = h
        .post(
            "tools.kryphocron.feed.createPostPrivate",
            Some(&access),
            json!({"repo": did, "record": private_post("x", v["uri"].as_str().unwrap())}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (st, _) = h
        .post("tools.kryphocron.policy.manageAudience", Some(&access),
            json!({"repo": did, "rkey": local_rkey, "record": {"members": ["did:plc:friend", "did:plc:two"]}}))
        .await;
    assert_eq!(st, StatusCode::OK);
    let row = h
        .state
        .store
        .kr_audience(&did, &local_rkey)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((row.pending.as_str(), row.row_version), ("none", 2));

    // A prism audience's edit is queued; its delete too.
    let (st, _) = h
        .post(
            "tools.kryphocron.policy.manageAudience",
            Some(&access),
            json!({"repo": did, "rkey": "3m6aaaaaaaaaa", "record": {"members": ["did:plc:x"]}}),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        h.state
            .store
            .kr_audience(&did, "3m6aaaaaaaaaa")
            .await
            .unwrap()
            .unwrap()
            .pending,
        "update"
    );

    // Proxied prism calls are answered from the store, in prism's shapes.
    let proxy = [("atproto-proxy", "did:web:prism.test#bsky_appview")];
    let (st, _, body) = h
        .raw(
            "GET",
            "/xrpc/app.nearhorizon.private.listAudiences?limit=10",
            Some(&access),
            vec![],
            &proxy,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let listed: Value = serde_json::from_slice(&body).unwrap();
    let rkeys: Vec<&str> = listed["audiences"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["uri"].as_str().unwrap().rsplit('/').next().unwrap())
        .collect();
    assert_eq!(
        rkeys,
        [local_rkey.as_str(), "3m6aaaaaaaaaa"],
        "newest first, local included"
    );
    let (st, _, body) = h
        .raw("POST", "/xrpc/app.nearhorizon.private.createAudience", Some(&access),
            serde_json::to_vec(&json!({"record": {"members": [], "createdAt": "2020-01-01T00:00:00Z", "updatedAt": "2020-01-01T00:00:00Z"}})).unwrap(),
            &[proxy[0], ("content-type", "application/json")])
        .await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let made: Value = serde_json::from_slice(&body).unwrap();
    let made_rkey = made["uri"].as_str().unwrap().rsplit('/').next().unwrap();
    assert_eq!(
        h.state
            .store
            .kr_audience(&did, made_rkey)
            .await
            .unwrap()
            .unwrap()
            .origin,
        "local"
    );

    let (st, _) = h
        .post(
            "tools.kryphocron.policy.deleteAudience",
            Some(&access),
            json!({"repo": did, "rkey": "3m6aaaaaaaaaa"}),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        h.state
            .store
            .kr_audience(&did, "3m6aaaaaaaaaa")
            .await
            .unwrap()
            .is_none(),
        "a pending delete counts as missing"
    );
    assert_eq!(
        h.state
            .store
            .audience_versions(&did)
            .await
            .unwrap()
            .iter()
            .find(|v| v.rkey == "3m6aaaaaaaaaa")
            .unwrap()
            .pending,
        "delete"
    );

    // With the tier off, a delete is still accepted (queued or local).
    h.state
        .settings
        .set("kryphocron.private_tier_enabled", json!(false), "test")
        .await
        .unwrap();
    let (st, _) = h
        .post(
            "tools.kryphocron.policy.deleteAudience",
            Some(&access),
            json!({"repo": did, "rkey": local_rkey}),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        h.state
            .store
            .kr_audience(&did, &local_rkey)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn an_ambiguous_create_is_linked_to_the_audience_a_refresh_adopts() {
    let h = harness(false).await;
    let acct = h
        .create_account("gil.pds.example.com", "gil@example.com", "pw")
        .await;
    let did = acct["did"].as_str().unwrap().to_owned();
    let (script, _) = h.with_prism().await;
    let stamp = "2024-05-01T10:00:00.123Z";
    let sent = json!({"$type": AUD, "members": ["did:plc:b", "did:plc:a"], "createdAt": stamp, "updatedAt": stamp});
    h.state
        .store
        .create_audience_attempt("att1", &did, &sent.to_string(), stamp, stamp, None)
        .await
        .unwrap();
    h.state
        .store
        .set_attempt_outcome("att1", "unknown")
        .await
        .unwrap();
    // Prism did create it (canonical form: members reordered, offset stamps).
    let mut entry = prism_audience(&did, "3m6madeitmade", "v1", &["did:plc:a", "did:plc:b"]);
    entry["record"]["createdAt"] = json!("2024-05-01T12:00:00.123+02:00");
    entry["record"]["updatedAt"] = json!("2024-05-01T12:00:00.123+02:00");
    set_script(
        &script,
        "app.nearhorizon.private.listAudiences",
        200,
        json!({"audiences": [entry]}),
    );
    let far = std::time::Instant::now() + std::time::Duration::from_secs(10);
    crate::audience::refresh(&h.state, &did, crate::audience::Trigger::Bypass, None, far)
        .await
        .unwrap();
    let row = h
        .state
        .store
        .attempt_row(&did, "att1")
        .await
        .unwrap()
        .expect("linked");
    assert_eq!(row.rkey, "3m6madeitmade");
    assert_eq!(
        h.state
            .store
            .audience_attempt("att1")
            .await
            .unwrap()
            .unwrap()
            .outcome,
        "adopted"
    );
}

#[tokio::test]
async fn outage_audiences_are_created_in_prism_mapped_and_posts_re_pointed() {
    let h = harness(false).await;
    let acct = h
        .create_account("hal.pds.example.com", "hal@example.com", "pw")
        .await;
    let (did, access) = (
        acct["did"].as_str().unwrap().to_owned(),
        acct["accessJwt"].as_str().unwrap().to_owned(),
    );
    let (script, seen) = h.with_prism().await;
    let existing = h.add_audience(&did, "3m6aaaaaaaaaa").await;
    let (st, _) = h
        .post(
            "tools.kryphocron.feed.createPostPrivate",
            Some(&access),
            json!({"repo": did, "record": private_post("x", &existing)}),
        )
        .await;
    assert_eq!(st, StatusCode::OK);

    // The outage: a local audience, a post under it, and a queued edit.
    h.state.kr.prism.set(true);
    let (st, v) = h
        .post(
            "tools.kryphocron.policy.manageAudience",
            Some(&access),
            json!({"repo": did, "record": {"members": ["did:plc:friend"]}}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let local_uri = v["uri"].as_str().unwrap().to_owned();
    let local = local_uri.rsplit('/').next().unwrap().to_owned();
    let (st, v) = h
        .post(
            "tools.kryphocron.feed.createPostPrivate",
            Some(&access),
            json!({"repo": did, "record": private_post("outage post", &local_uri)}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let post_rkey = v["uri"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    let before = h.record_value(&did, POST_PRIVATE, &post_rkey).await;
    assert_eq!(before["audienceList"], json!(local_uri));
    let (st, _) = h
        .post(
            "tools.kryphocron.policy.manageAudience",
            Some(&access),
            json!({"repo": did, "rkey": "3m6aaaaaaaaaa", "record": {"members": ["did:plc:added"]}}),
        )
        .await;
    assert_eq!(st, StatusCode::OK);

    // Prism is back. Its listing has the prism audience unchanged since
    // the outage began.
    h.state.kr.prism.set(false);
    set_script(
        &script,
        "app.nearhorizon.private.listAudiences",
        200,
        json!({"audiences": [prism_audience(&did, "3m6aaaaaaaaaa", "3m6prismcid22", &[])]}),
    );
    set_script(
        &script,
        "app.nearhorizon.private.updateAudience",
        200,
        json!({"uri": existing, "cid": "after-push"}),
    );
    let prism_uri = format!("at://{did}/{AUD}/3m6pmappedmap");
    set_script(
        &script,
        "app.nearhorizon.private.createAudience",
        200,
        json!({"uri": prism_uri, "cid": "c1"}),
    );
    h.state.store.poll_queued_audiences_now().await.unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let polled = crate::audience_sync::poll_once(&h.state, &cancel)
        .await
        .unwrap();
    assert_eq!(polled, 1);

    // The queued edit was pushed (newer than prism's), CAS on prism's token.
    let a = h
        .state
        .store
        .kr_audience(&did, "3m6aaaaaaaaaa")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            a.pending.as_str(),
            a.prism_cid.as_deref(),
            a.members_json.as_str()
        ),
        ("none", Some("after-push"), "[\"did:plc:added\"]")
    );
    let pushed: Value = {
        let log = seen.lock().unwrap();
        serde_json::from_slice(
            &log.iter()
                .rev()
                .find(|s| s.uri.contains("updateAudience"))
                .unwrap()
                .body,
        )
        .unwrap()
    };
    assert_eq!(pushed["swapCid"], json!("3m6prismcid22"));

    // The local audience was created in prism with its own stamps, and
    // mapped: alias, prism row, local row gone.
    let created: Value = {
        let log = seen.lock().unwrap();
        serde_json::from_slice(
            &log.iter()
                .rev()
                .find(|s| s.uri.contains("createAudience"))
                .unwrap()
                .body,
        )
        .unwrap()
    };
    assert_eq!(created["record"]["members"], json!(["did:plc:friend"]));
    assert!(created["record"]["createdAt"].is_string());
    assert_eq!(
        h.state
            .store
            .audience_alias(&did, &local)
            .await
            .unwrap()
            .as_deref(),
        Some("3m6pmappedmap")
    );
    assert!(
        h.state
            .store
            .audience_versions(&did)
            .await
            .unwrap()
            .iter()
            .all(|v| v.rkey != local)
    );
    assert_eq!(
        h.state
            .store
            .kr_audience(&did, "3m6pmappedmap")
            .await
            .unwrap()
            .unwrap()
            .prism_cid
            .as_deref(),
        Some("c1")
    );
    // A mapping applied twice is a no-op.
    assert!(
        !h.state
            .store
            .map_local_audience(&did, &local, None, "3m6pmappedmap", None)
            .await
            .unwrap()
    );
    // The local URI still resolves (to the prism audience).
    let (st, v) = h
        .post(
            "tools.kryphocron.feed.createPostPrivate",
            Some(&access),
            json!({"repo": did, "record": private_post("later", &local_uri)}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");

    // Re-pointing: the outage post now names the prism URI, content intact.
    h.state
        .settings
        .set(
            "kryphocron.repoint_max_commits_per_minute",
            json!(10_000),
            "test",
        )
        .await
        .unwrap();
    let moved = crate::audience_sync::repoint_pass(&h.state, &cancel)
        .await
        .unwrap();
    assert_eq!(moved, 1);
    let after = h.record_value(&did, POST_PRIVATE, &post_rkey).await;
    assert_eq!(after["audienceList"], json!(prism_uri));
    assert_eq!(after["encodedContent"], before["encodedContent"]);
    assert_eq!(
        after["encodedContentGeneration"],
        before["encodedContentGeneration"]
    );
    assert_eq!(
        crate::audience_sync::repoint_pass(&h.state, &cancel)
            .await
            .unwrap(),
        0,
        "nothing left"
    );
}

#[tokio::test]
async fn a_queued_edit_loses_to_a_newer_prism_version() {
    let h = harness(false).await;
    let acct = h
        .create_account("ivy.pds.example.com", "ivy@example.com", "pw")
        .await;
    let (did, access) = (
        acct["did"].as_str().unwrap().to_owned(),
        acct["accessJwt"].as_str().unwrap().to_owned(),
    );
    let (script, _) = h.with_prism().await;
    let existing = h.add_audience(&did, "3m6aaaaaaaaaa").await;
    let (st, _) = h
        .post(
            "tools.kryphocron.feed.createPostPrivate",
            Some(&access),
            json!({"repo": did, "record": private_post("x", &existing)}),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    h.state.kr.prism.set(true);
    let (st, _) = h
        .post(
            "tools.kryphocron.policy.manageAudience",
            Some(&access),
            json!({"repo": did, "rkey": "3m6aaaaaaaaaa", "record": {"members": ["did:plc:mine"]}}),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    h.state.kr.prism.set(false);
    // Prism changed it later than the queued edit.
    let mut theirs = prism_audience(&did, "3m6aaaaaaaaaa", "newer", &["did:plc:theirs"]);
    theirs["record"]["updatedAt"] = json!("2999-01-01T00:00:00.000Z");
    set_script(
        &script,
        "app.nearhorizon.private.listAudiences",
        200,
        json!({"audiences": [theirs]}),
    );
    h.state.store.poll_queued_audiences_now().await.unwrap();
    crate::audience_sync::poll_once(&h.state, &tokio_util::sync::CancellationToken::new())
        .await
        .unwrap();
    let a = h
        .state
        .store
        .kr_audience(&did, "3m6aaaaaaaaaa")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            a.pending.as_str(),
            a.members_json.as_str(),
            a.prism_cid.as_deref()
        ),
        ("none", "[\"did:plc:theirs\"]", Some("newer"))
    );
}

impl Harness {
    async fn add_audience_with(
        &self,
        did: &str,
        rkey: &str,
        mode: &str,
        members: &[&str],
    ) -> String {
        let uri = self.add_audience(did, rkey).await;
        let mut row = self
            .state
            .store
            .kr_audience(did, rkey)
            .await
            .unwrap()
            .unwrap();
        row.mode = mode.into();
        row.members_json = serde_json::to_string(members).unwrap();
        self.state.store.put_kr_audience(&row).await.unwrap();
        // A copy just checked, so membership answers from it.
        self.state
            .store
            .note_listing(did, self.state.store.db_now().await.unwrap(), true, None)
            .await
            .unwrap();
        uri
    }

    async fn user(&self, handle: &str) -> (String, String) {
        let a = self
            .create_account(
                handle,
                &format!("{}@example.com", handle.split('.').next().unwrap()),
                "pw",
            )
            .await;
        (
            a["did"].as_str().unwrap().to_owned(),
            a["accessJwt"].as_str().unwrap().to_owned(),
        )
    }

    async fn private_post_as(&self, did: &str, access: &str, record: Value) -> (StatusCode, Value) {
        self.post(
            "tools.kryphocron.feed.createPostPrivate",
            Some(access),
            json!({"repo": did, "record": record}),
        )
        .await
    }

    async fn reply_as(
        &self,
        did: &str,
        access: &str,
        parent: &Value,
        audience: &str,
    ) -> (StatusCode, Value) {
        let mut rec = private_post("a reply", audience);
        rec["reply"] = json!({"root": {"uri": parent["uri"], "cid": parent["cid"]}, "parent": {"uri": parent["uri"], "cid": parent["cid"]}});
        self.post(
            "tools.kryphocron.actor.participatePrivate",
            Some(access),
            json!({"repo": did, "record": rec}),
        )
        .await
    }
}

#[tokio::test]
async fn private_replies_go_under_the_parents_audience_for_its_members() {
    let h = harness(false).await;
    let (owner, owner_access) = h.user("oona.pds.example.com").await;
    let (writer, writer_access) = h.user("wren.pds.example.com").await;
    let (other, other_access) = h.user("otto.pds.example.com").await;
    let (script, _) = h.with_prism().await;
    let audience = h
        .add_audience_with(&owner, "3m6aaaaaaaaaa", "list", &[&writer])
        .await;
    let (st, parent) = h
        .private_post_as(&owner, &owner_access, private_post("the parent", &audience))
        .await;
    assert_eq!(st, StatusCode::OK, "{parent}");
    // The writer's own first private-tier write makes them a known member.
    let mine = h.add_audience(&writer, "3m6wwwwwwwwww").await;
    assert_eq!(
        h.private_post_as(&writer, &writer_access, private_post("x", &mine))
            .await
            .0,
        StatusCode::OK
    );
    let mine_other = h.add_audience(&other, "3m6oooooooooo").await;
    assert_eq!(
        h.private_post_as(&other, &other_access, private_post("x", &mine_other))
            .await
            .0,
        StatusCode::OK
    );

    // A member replies; the reply is stored encoded under the parent's audience.
    let (st, v) = h
        .reply_as(&writer, &writer_access, &parent, &audience)
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let reply = h
        .record_value(
            &writer,
            POST_PRIVATE,
            v["uri"].as_str().unwrap().rsplit('/').next().unwrap(),
        )
        .await;
    assert_eq!(reply["audienceList"], json!(audience));
    assert!(reply.get("text").is_none() && reply["reply"]["parent"]["uri"] == parent["uri"]);

    // Not in the list, or naming another audience: the same Forbidden.
    let (st, v) = h.reply_as(&other, &other_access, &parent, &audience).await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("Forbidden"))
    );
    let (st, v2) = h.reply_as(&writer, &writer_access, &parent, &mine).await;
    assert_eq!((st, &v2), (StatusCode::FORBIDDEN, &v), "identical refusals");
    // The generic endpoints run the same checks.
    let mut rec = private_post("generic reply", &audience);
    rec["reply"] = json!({"root": {"uri": parent["uri"], "cid": parent["cid"]}, "parent": {"uri": parent["uri"], "cid": parent["cid"]}});
    let (st, _) = h
        .post(
            "com.atproto.repo.createRecord",
            Some(&other_access),
            json!({"repo": other, "collection": POST_PRIVATE, "record": rec}),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // A mapped local URI is the same audience, and the reply names prism's.
    h.state
        .store
        .put_audience_alias(&owner, "3m6llllllllll", "3m6aaaaaaaaaa")
        .await
        .unwrap();
    let local_uri = format!("at://{owner}/{AUD}/3m6llllllllll");
    let (st, v) = h
        .reply_as(&writer, &writer_access, &parent, &local_uri)
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let reply = h
        .record_value(
            &writer,
            POST_PRIVATE,
            v["uri"].as_str().unwrap().rsplit('/').next().unwrap(),
        )
        .await;
    assert_eq!(reply["audienceList"], json!(audience));

    // A member who isn't ready in prism is refused.
    let mut row = h
        .state
        .store
        .kr_permission(&writer, "membership")
        .await
        .unwrap()
        .unwrap();
    row.private_read = false;
    h.state.store.put_kr_permission(&row).await.unwrap();
    assert_eq!(
        h.reply_as(&writer, &writer_access, &parent, &audience)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    row.private_read = true;
    h.state.store.put_kr_permission(&row).await.unwrap();

    // A block either way.
    let (st, _) = h
        .post(
            "com.atproto.repo.createRecord",
            Some(&owner_access),
            json!({"repo": owner, "collection": "app.bsky.graph.block",
        "record": {"subject": writer, "createdAt": "2024-01-01T00:00:00.000Z"}}),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        h.reply_as(&writer, &writer_access, &parent, &audience)
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    // The root's threadgate: nobody but mentions.
    let (owner2, owner2_access) = h.user("pia.pds.example.com").await;
    let aud2 = h
        .add_audience_with(&owner2, "3m6bbbbbbbbbb", "list", &[&writer])
        .await;
    let mut gated = private_post("gated", &aud2);
    gated["facets"] = json!([{"index": {"byteStart": 0, "byteEnd": 1},
        "features": [{"$type": "app.bsky.richtext.facet#mention", "did": writer}]}]);
    let (_, p2) = h.private_post_as(&owner2, &owner2_access, gated).await;
    let p2_rkey = p2["uri"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    let gate = |allow: Value| {
        let (h, a, d, p2, rkey) = (
            &h,
            owner2_access.clone(),
            owner2.clone(),
            p2.clone(),
            p2_rkey.clone(),
        );
        async move {
            h.post("com.atproto.repo.putRecord", Some(&a), json!({"repo": d, "collection": "tools.kryphocron.feed.threadgate", "rkey": rkey,
                "record": {"post": {"uri": p2["uri"], "cid": p2["cid"]}, "allow": allow, "createdAt": "2024-01-01T00:00:00.000Z"}})).await
        }
    };
    assert_eq!(gate(json!([])).await.0, StatusCode::OK);
    assert_eq!(
        h.reply_as(&writer, &writer_access, &p2, &aud2).await.0,
        StatusCode::FORBIDDEN,
        "no one may reply"
    );
    assert_eq!(
        gate(json!([{"$type": "tools.kryphocron.feed.threadgate#mentionRule"}]))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        h.reply_as(&writer, &writer_access, &p2, &aud2).await.0,
        StatusCode::OK,
        "mentioned"
    );

    // Non-list modes: only a post prism served counts.
    let (owner3, owner3_access) = h.user("quin.pds.example.com").await;
    let aud3 = h
        .add_audience_with(&owner3, "3m6cccccccccc", "everyone", &[])
        .await;
    let (_, p3) = h
        .private_post_as(&owner3, &owner3_access, private_post("for everyone", &aud3))
        .await;
    set_script(
        &script,
        "app.nearhorizon.private.getPosts",
        200,
        json!({"posts": []}),
    );
    assert_eq!(
        h.reply_as(&writer, &writer_access, &p3, &aud3).await.0,
        StatusCode::FORBIDDEN,
        "a miss proves nothing"
    );
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    h.state.kr.membership.clear_probes();
    set_script(
        &script,
        "app.nearhorizon.private.getPosts",
        200,
        json!({"posts": [{"uri": p3["uri"], "record": {"text": "dropped"}}]}),
    );
    assert_eq!(
        h.reply_as(&writer, &writer_access, &p3, &aud3).await.0,
        StatusCode::OK,
        "served"
    );
}

#[tokio::test]
async fn membership_assertions_are_issued_to_members_and_verified() {
    let h = harness(false).await;
    let (owner, _) = h.user("rae.pds.example.com").await;
    let (member, member_access) = h.user("sam.pds.example.com").await;
    let (stranger, stranger_access) = h.user("tia.pds.example.com").await;
    h.with_prism().await;
    let audience = h
        .add_audience_with(&owner, "3m6aaaaaaaaaa", "list", &[&member])
        .await;
    // The member is a known ready member.
    let mine = h.add_audience(&member, "3m6mmmmmmmmmm").await;
    assert_eq!(
        h.private_post_as(&member, &member_access, private_post("x", &mine))
            .await
            .0,
        StatusCode::OK
    );
    let mine2 = h.add_audience(&stranger, "3m6ssssssssss").await;
    assert_eq!(
        h.private_post_as(&stranger, &stranger_access, private_post("x", &mine2))
            .await
            .0,
        StatusCode::OK
    );

    let ask = |access: String| {
        let (h, audience) = (&h, audience.clone());
        async move {
            let (_, t) = h
                .service_token(
                    &access,
                    "did:web:pds.example.com",
                    crate::membership::ASSERTION_LXM,
                )
                .await;
            let token = t["token"].as_str().unwrap().to_owned();
            let path = format!(
                "/xrpc/{}?audienceUri={}&aud={}",
                crate::membership::ASSERTION_LXM,
                url::form_urlencoded::byte_serialize(audience.as_bytes()).collect::<String>(),
                url::form_urlencoded::byte_serialize(b"did:web:pds.example.com")
                    .collect::<String>(),
            );
            h.call("GET", &path, Some(&token), None).await
        }
    };
    let (st, v) = ask(member_access.clone()).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let assertion = v["token"].as_str().unwrap().to_owned();
    let (st, v) = ask(stranger_access).await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("Forbidden"))
    );

    // Verified against the issuer's #kryphocron_audience key, its claims bound.
    let (_, doc) = h
        .call_host(
            "GET",
            "/.well-known/did.json",
            None,
            None,
            "pds.example.com",
        )
        .await;
    let key = doc["verificationMethod"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"].as_str().unwrap().ends_with("#kryphocron_audience"))
        .unwrap()["publicKeyMultibase"]
        .as_str()
        .unwrap()
        .to_owned();
    h.state
        .kr
        .membership
        .seed_issuer_key("pds.example.com", &format!("did:key:{key}"));
    assert!(crate::membership::verify_assertion(&h.state, &assertion, &member, &audience).await);
    assert!(
        !crate::membership::verify_assertion(&h.state, &assertion, &stranger, &audience).await,
        "sub is bound"
    );
    let other_aud = format!("at://{owner}/{AUD}/3m6zzzzzzzzzz");
    assert!(
        !crate::membership::verify_assertion(&h.state, &assertion, &member, &other_aud).await,
        "audienceUri is bound"
    );
    h.state
        .kr
        .membership
        .seed_issuer_key("pds.example.com", &h.state.service_did_key());
    assert!(
        !crate::membership::verify_assertion(&h.state, &assertion, &member, &audience).await,
        "the service key doesn't sign them"
    );

    // An assertion never passes as service auth.
    assert!(matches!(
        locus_auth::service_jwt::parse(&assertion, None, 0),
        Err(locus_auth::service_jwt::ServiceJwtError::BadJwtType)
    ));
}

/// A scanner webhook: matches bytes containing `BAD`; answers 500 while
/// `down` is set.
async fn mock_scanner() -> (String, Arc<std::sync::atomic::AtomicBool>) {
    let down = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let app = Router::new()
        .route(
            "/scan",
            axum::routing::post(
                |State(down): State<Arc<std::sync::atomic::AtomicBool>>,
                 body: axum::body::Bytes| async move {
                    if down.load(std::sync::atomic::Ordering::SeqCst) {
                        return (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(json!({})))
                            .into_response();
                    }
                    let bad = body.windows(3).any(|w| w == b"BAD");
                    let result = if bad { "quarantine" } else { "allow" };
                    axum::Json(json!({"result": result, "reason": "test hash"})).into_response()
                },
            ),
        )
        .with_state(down.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/scan"), down)
}

#[tokio::test]
async fn uploads_are_scanned_and_matches_quarantined_for_review() {
    let h = harness(false).await;
    let (did, access) = h.user("una.pds.example.com").await;
    let (url, down) = mock_scanner().await;
    h.state
        .settings
        .set("blobs.scanner", json!("webhook"), "test")
        .await
        .unwrap();
    h.state
        .settings
        .set("blobs.scanner_url", json!(url), "test")
        .await
        .unwrap();

    let good = h.upload(&access, b"a good picture", "image/png").await;
    let good_cid = good["ref"]["$link"].as_str().unwrap().to_owned();
    assert_eq!(
        h.state
            .store
            .scan_status(&did, &good_cid)
            .await
            .unwrap()
            .as_deref(),
        Some("allowed")
    );

    // A match is refused, kept (held, quarantined) and queued for review.
    let bad_bytes = b"a BAD picture";
    let bad_cid = proto_blue_lex_data::Cid::for_raw(bad_bytes).to_string_base32();
    let (st, _, body) = h
        .raw(
            "POST",
            "/xrpc/com.atproto.repo.uploadBlob",
            Some(&access),
            bad_bytes.to_vec(),
            &[("content-type", "image/png")],
        )
        .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let row = h.state.store.blob(&did, &bad_cid).await.unwrap().unwrap();
    assert_eq!((row.state.as_str(), row.review_hold), ("quarantined", true));
    assert!(
        row.storage_key
            .starts_with(&format!("quarantine/{did}/{bad_cid}.")),
        "preserved in quarantine: {row:?}"
    );
    assert!(
        h.state
            .blobs
            .size(&row.storage_key)
            .await
            .unwrap()
            .is_some()
    );
    let reviews = h.state.store.reviews("open", 10).await.unwrap();
    assert_eq!(
        (
            reviews.len(),
            reviews[0].cid.as_str(),
            reviews[0].source.as_str()
        ),
        (1, bad_cid.as_str(), "upload")
    );
    let (st, _, _) = h
        .raw_get(format!(
            "/xrpc/com.atproto.sync.getBlob?did={did}&cid={bad_cid}"
        ))
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "not served");
    let (st, _, _) = h
        .raw(
            "POST",
            "/xrpc/com.atproto.repo.uploadBlob",
            Some(&access),
            bad_bytes.to_vec(),
            &[("content-type", "image/png")],
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "can't be uploaded again");

    // The scanner is down: refused (retryable) by default...
    down.store(true, std::sync::atomic::Ordering::SeqCst);
    let (st, _, _) = h
        .raw(
            "POST",
            "/xrpc/com.atproto.repo.uploadBlob",
            Some(&access),
            b"while down".to_vec(),
            &[("content-type", "image/png")],
        )
        .await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
    // ...or stored unscanned and scanned when it's back.
    h.state
        .settings
        .set("blobs.scan_unavailable", json!("accept_unscanned"), "test")
        .await
        .unwrap();
    let later_ok = h.upload(&access, b"fine, checked later", "image/png").await;
    let later_bad = h.upload(&access, b"BAD, checked later", "image/png").await;
    let (ok_cid, bad2_cid) = (
        later_ok["ref"]["$link"].as_str().unwrap().to_owned(),
        later_bad["ref"]["$link"].as_str().unwrap().to_owned(),
    );
    assert_eq!(
        h.state
            .store
            .scan_status(&did, &ok_cid)
            .await
            .unwrap()
            .as_deref(),
        Some("unscanned")
    );
    assert_eq!(
        crate::scanner::rescan_pass(&h.state).await.unwrap(),
        0,
        "still down"
    );
    down.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(crate::scanner::rescan_pass(&h.state).await.unwrap(), 2);
    assert_eq!(
        h.state
            .store
            .scan_status(&did, &ok_cid)
            .await
            .unwrap()
            .as_deref(),
        Some("allowed")
    );
    assert_eq!(
        h.state
            .store
            .blob(&did, &bad2_cid)
            .await
            .unwrap()
            .unwrap()
            .state,
        "quarantined"
    );
    assert_eq!(h.state.store.reviews("open", 10).await.unwrap().len(), 2);
    let row = h.state.store.blob(&did, &bad2_cid).await.unwrap().unwrap();
    assert!(row.storage_key.starts_with("quarantine/"), "{row:?}");
    // Cleared as a false positive: out of quarantine (back to temp, as
    // nothing references it).
    h.state
        .store
        .release_quarantined_blob(&did, &bad2_cid)
        .await
        .unwrap();
    crate::repo::place_blob(&h.state, &did, &bad2_cid).await;
    let row = h.state.store.blob(&did, &bad2_cid).await.unwrap().unwrap();
    assert!(row.storage_key.starts_with("tmp/"), "{row:?}");

    // An operator resolves a review once.
    let id = h.state.store.reviews("open", 10).await.unwrap()[0].id;
    assert!(
        h.state
            .store
            .resolve_review(id, "cleared", "admin", Some("false positive"))
            .await
            .unwrap()
    );
    assert!(
        !h.state
            .store
            .resolve_review(id, "reported", "admin", None)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn gated_private_media_is_served_only_to_the_audience() {
    let h = harness(false).await;
    let (owner, owner_access) = h.user("vic.pds.example.com").await;
    let (member, member_access) = h.user("wes.pds.example.com").await;
    let (stranger, stranger_access) = h.user("xan.pds.example.com").await;
    h.with_prism().await;
    let (url, _) = mock_scanner().await;
    h.state
        .settings
        .set("blobs.scanner", json!("webhook"), "test")
        .await
        .unwrap();
    h.state
        .settings
        .set("blobs.scanner_url", json!(url), "test")
        .await
        .unwrap();
    let audience = h
        .add_audience_with(&owner, "3m6aaaaaaaaaa", "list", &[&member])
        .await;
    for (did, access, rkey) in [
        (&member, &member_access, "3m6mmmmmmmmmm"),
        (&stranger, &stranger_access, "3m6ssssssssss"),
    ] {
        let own = h.add_audience(did, rkey).await;
        assert_eq!(
            h.private_post_as(did, access, private_post("x", &own))
                .await
                .0,
            StatusCode::OK
        );
    }

    let img = h
        .upload(&owner_access, b"\x89PNG a private picture", "image/png")
        .await;
    let cid = img["ref"]["$link"].as_str().unwrap().to_owned();
    let mut rec = private_post("a picture", &audience);
    rec["embed"] = json!({"$type": "tools.kryphocron.feed.postPublic#embedImage", "images": [{"image": img, "alt": ""}]});
    let (st, v) = h.private_post_as(&owner, &owner_access, rec).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(h.state.store.blob_private_only(&owner, &cid).await.unwrap());

    let path = format!("/xrpc/com.atproto.sync.getBlob?did={owner}&cid={cid}");
    let missing = format!(
        "/xrpc/com.atproto.sync.getBlob?did={owner}&cid={}",
        proto_blue_lex_data::Cid::for_raw(b"never uploaded").to_string_base32()
    );
    let get = |auth: Option<String>, p: String| {
        let h = &h;
        async move { h.raw("GET", &p, auth.as_deref(), vec![], &[]).await }
    };
    // Gate off (the default): served by CID like any blob.
    assert_eq!(get(None, path.clone()).await.0, StatusCode::OK);

    let key_of = |cid: String| {
        let h = &h;
        let owner = owner.clone();
        async move {
            let r = h.state.store.blob(&owner, &cid).await.unwrap().unwrap();
            assert!(r.pending_move.is_none(), "{r:?}");
            r.storage_key
        }
    };
    assert!(
        key_of(cid.clone()).await.starts_with("blobs/"),
        "gate off: the normal store"
    );

    h.state
        .settings
        .set("kryphocron.private_media_gated", json!(true), "test")
        .await
        .unwrap();
    // Switching the gate on moves private-only bytes to the gated store.
    assert_eq!(
        locus_blob::run_placement(&h.state.store, &h.state.blobs, true, 100)
            .await
            .unwrap(),
        1
    );
    assert!(
        key_of(cid.clone())
            .await
            .starts_with(&format!("gated/{owner}/{cid}."))
    );
    assert_eq!(
        locus_blob::run_placement(&h.state.store, &h.state.blobs, true, 100)
            .await
            .unwrap(),
        0,
        "nothing left to move"
    );
    // With the gate on, new private media goes straight there.
    let img2 = h
        .upload(&owner_access, b"\x89PNG another private one", "image/png")
        .await;
    let cid2 = img2["ref"]["$link"].as_str().unwrap().to_owned();
    let mut rec = private_post("another", &audience);
    rec["embed"] = json!({"$type": "tools.kryphocron.feed.postPublic#embedImage", "images": [{"image": img2, "alt": ""}]});
    let (st, v) = h.private_post_as(&owner, &owner_access, rec).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(key_of(cid2.clone()).await.starts_with("gated/"));
    let (st, headers, body) = get(None, path.clone()).await;
    let (mst, mheaders, mbody) = get(None, missing.clone()).await;
    assert_eq!((st, &body), (mst, &mbody), "identical to a missing blob");
    for h in [&headers, &mheaders] {
        assert_eq!(h["cache-control"], "private, no-store");
        assert_eq!(h["vary"], "Authorization");
    }
    assert_eq!(
        get(Some(stranger_access.clone()), path.clone()).await.0,
        StatusCode::BAD_REQUEST,
        "not in the audience"
    );
    let (st, headers, body) = get(Some(member_access.clone()), path.clone()).await;
    assert_eq!(
        (st, body.as_slice()),
        (StatusCode::OK, &b"\x89PNG a private picture"[..])
    );
    assert_eq!(headers["cache-control"], "private, no-store");
    assert_eq!(
        get(Some(owner_access.clone()), path.clone()).await.0,
        StatusCode::OK
    );

    // A trusted service, with service auth; every fetch is audited.
    h.state
        .settings
        .set(
            "kryphocron.media_trusted_services",
            json!([stranger]),
            "test",
        )
        .await
        .unwrap();
    let (_, t) = h
        .service_token(
            &stranger_access,
            &h.state.service_did,
            "com.atproto.sync.getBlob",
        )
        .await;
    assert_eq!(
        get(Some(t["token"].as_str().unwrap().to_owned()), path.clone())
            .await
            .0,
        StatusCode::OK
    );
    // The audit writer runs in the background: wait for it, briefly.
    let mut audited = false;
    for _ in 0..50 {
        audited = h
            .state
            .store
            .kr_audit(50)
            .await
            .unwrap()
            .iter()
            .any(|r| r.event == "media.trusted_fetch");
        if audited {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(audited, "the trusted fetch is audited");
    h.state
        .settings
        .set("kryphocron.media_trusted_services", json!([]), "test")
        .await
        .unwrap();

    // listBlobs leaves it out, except for the owner.
    let list = format!("/xrpc/com.atproto.sync.listBlobs?did={owner}");
    let (_, anyone) = h.call("GET", &list, None, None).await;
    assert!(
        !anyone["cids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == &json!(cid)),
        "{anyone}"
    );
    let (_, mine) = h.call("GET", &list, Some(&owner_access), None).await;
    assert!(
        mine["cids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == &json!(cid))
    );

    // A public record referencing it makes it public at once.
    let mut public = post_record("now public");
    public["embed"] =
        json!({"$type": "app.bsky.embed.images", "images": [{"image": img, "alt": ""}]});
    let (st, v) = h
        .post(
            "com.atproto.repo.createRecord",
            Some(&owner_access),
            json!({"repo": owner, "collection": "app.bsky.feed.post", "record": public}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(!h.state.store.blob_private_only(&owner, &cid).await.unwrap());
    assert!(
        key_of(cid.clone()).await.starts_with("blobs/"),
        "public: out of the gated store"
    );
    let (st, headers, _) = get(None, path.clone()).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        headers
            .get("cache-control")
            .is_none_or(|v| v != "private, no-store")
    );

    // Switching the gate off moves gated bytes back to the normal store.
    h.state
        .settings
        .set("kryphocron.private_media_gated", json!(false), "test")
        .await
        .unwrap();
    assert!(key_of(cid2.clone()).await.starts_with("gated/"));
    assert_eq!(
        locus_blob::run_placement(&h.state.store, &h.state.blobs, false, 100)
            .await
            .unwrap(),
        1
    );
    assert!(key_of(cid2.clone()).await.starts_with("blobs/"));
    let (st, _, body) = get(
        None,
        format!("/xrpc/com.atproto.sync.getBlob?did={owner}&cid={cid2}"),
    )
    .await;
    assert_eq!(
        (st, body.as_slice()),
        (StatusCode::OK, &b"\x89PNG another private one"[..])
    );
}

// ---------------------------------------------------------------- admin

impl Harness {
    /// An operator with an API token.
    async fn operator(&self, name: &str, role: &str) -> String {
        assert!(
            self.state
                .store
                .create_operator(name, "x", role)
                .await
                .unwrap()
        );
        let token = crate::admin_auth::new_token();
        self.state
            .store
            .create_operator_token(
                &format!("t-{name}"),
                &crate::admin_auth::token_hash(&token),
                name,
                "test",
                None,
            )
            .await
            .unwrap();
        token
    }

    fn basic_admin(&self) -> String {
        let pw = String::from_utf8(self.state.secrets.admin_password.to_vec()).unwrap();
        format!(
            "Basic {}",
            base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                format!("admin:{pw}")
            )
        )
    }

    async fn admin_call(
        &self,
        method: &str,
        path: &str,
        auth: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let bytes = body
            .map(|b| serde_json::to_vec(&b).unwrap())
            .unwrap_or_default();
        let (st, _, out) = self
            .raw(
                method,
                path,
                None,
                bytes,
                &[
                    ("authorization", auth),
                    ("content-type", "application/json"),
                ],
            )
            .await;
        (st, serde_json::from_slice(&out).unwrap_or(Value::Null))
    }
}

#[tokio::test]
async fn the_admin_api_checks_roles_and_takes_subjects_down() {
    let h = harness(false).await;
    let (did, access) = h.user("yara.pds.example.com").await;
    let viewer = format!("Bearer {}", h.operator("vee", "viewer").await);
    let moderator = format!("Bearer {}", h.operator("mo", "moderator").await);
    let admin = format!("Bearer {}", h.operator("ad", "admin").await);
    let info = format!("/xrpc/com.atproto.admin.getAccountInfo?did={did}");

    // Who may call what.
    let (st, v) = h.admin_call("GET", &info, &viewer, None).await;
    assert_eq!(
        (st, v["handle"].as_str()),
        (StatusCode::OK, Some("yara.pds.example.com")),
        "{v}"
    );
    assert_eq!(v["email"], "yara@example.com");
    assert_eq!(
        h.call("GET", &info, None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        h.admin_call("GET", &info, "Basic YWRtaW46d3Jvbmc=", None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        h.admin_call("GET", &info, &h.basic_admin(), None).await.0,
        StatusCode::OK
    );
    let takedown = |applied: bool| json!({"subject": {"$type": "com.atproto.admin.defs#repoRef", "did": did}, "takedown": {"applied": applied, "ref": "case-1"}});
    let update = "/xrpc/com.atproto.admin.updateSubjectStatus";
    assert_eq!(
        h.admin_call("POST", update, &viewer, Some(takedown(true)))
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    // An account takedown ends its sessions and hides its repo, except
    // from admin credentials.
    let outbox_before = h.state.store.outbox_for(&did).await.unwrap().len();
    let (st, v) = h
        .admin_call("POST", update, &moderator, Some(takedown(true)))
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(
        !h.call(
            "GET",
            "/xrpc/com.atproto.server.getSession",
            Some(&access),
            None
        )
        .await
        .0
        .is_success()
    );
    assert_eq!(
        h.state.store.outbox_for(&did).await.unwrap().len(),
        outbox_before + 1,
        "#account"
    );
    let status = format!("/xrpc/com.atproto.admin.getSubjectStatus?did={did}");
    let (_, v) = h.admin_call("GET", &status, &viewer, None).await;
    assert_eq!(v["takedown"], json!({"applied": true, "ref": "case-1"}));
    let repo = format!("/xrpc/com.atproto.sync.getRepo?did={did}");
    let (st, _, _) = h.raw_get(repo.clone()).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "RepoTakendown");
    let (st, _, _) = h
        .raw(
            "GET",
            &repo,
            None,
            vec![],
            &[("authorization", &h.basic_admin())],
        )
        .await;
    assert_eq!(st, StatusCode::OK, "admin credentials read it");
    assert_eq!(
        h.admin_call("POST", update, &moderator, Some(takedown(false)))
            .await
            .0,
        StatusCode::OK
    );
    let (_, v) = h.admin_call("GET", &status, &viewer, None).await;
    assert_eq!(v["takedown"], json!({"applied": false}));

    // Records, by URI whatever the CID; blobs.
    let access = h.create_session("yara.pds.example.com", "pw").await;
    let (st, rec) = h
        .post("com.atproto.repo.createRecord", Some(&access), json!({"repo": did, "collection": "app.bsky.feed.post", "record": post_record("hide me")}))
        .await;
    assert_eq!(st, StatusCode::OK, "{rec}");
    let uri = rec["uri"].as_str().unwrap().to_owned();
    let rkey = uri.rsplit('/').next().unwrap().to_owned();
    let subject = json!({"subject": {"$type": "com.atproto.repo.strongRef", "uri": uri, "cid": "bafyanything"}, "takedown": {"applied": true, "ref": "case-2"}});
    assert_eq!(
        h.admin_call("POST", update, &moderator, Some(subject))
            .await
            .0,
        StatusCode::OK
    );
    let (st, v) = h
        .get(format!(
            "/xrpc/com.atproto.repo.getRecord?repo={did}&collection=app.bsky.feed.post&rkey={rkey}"
        ))
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("RecordNotFound"))
    );
    let (_, list) = h
        .get(format!(
            "/xrpc/com.atproto.repo.listRecords?repo={did}&collection=app.bsky.feed.post"
        ))
        .await;
    assert!(list["records"].as_array().unwrap().is_empty());
    let enc = url::form_urlencoded::byte_serialize(uri.as_bytes()).collect::<String>();
    let (_, v) = h
        .admin_call(
            "GET",
            &format!("/xrpc/com.atproto.admin.getSubjectStatus?uri={enc}"),
            &viewer,
            None,
        )
        .await;
    assert_eq!(v["takedown"]["applied"], true);
    let blob = h.upload(&access, b"some bytes", "image/png").await;
    let cid = blob["ref"]["$link"].as_str().unwrap().to_owned();
    let subject = json!({"subject": {"$type": "com.atproto.admin.defs#repoBlobRef", "did": did, "cid": cid}, "takedown": {"applied": true}});
    assert_eq!(
        h.admin_call("POST", update, &moderator, Some(subject))
            .await
            .0,
        StatusCode::OK
    );
    let (st, _, _) = h
        .raw_get(format!(
            "/xrpc/com.atproto.sync.getBlob?did={did}&cid={cid}"
        ))
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "taken down");
    // Its bytes moved to quarantine, and the old copy is queued.
    let row = h.state.store.blob(&did, &cid).await.unwrap().unwrap();
    assert!(
        row.storage_key
            .starts_with(&format!("quarantine/{did}/{cid}."))
            && row.pending_move.is_none(),
        "{row:?}"
    );
    assert_eq!(
        h.state.blobs.size(&row.storage_key).await.unwrap(),
        Some(10)
    );
    // Reversed: back out of quarantine, served again.
    let subject = json!({"subject": {"$type": "com.atproto.admin.defs#repoBlobRef", "did": did, "cid": cid}, "takedown": {"applied": false}});
    assert_eq!(
        h.admin_call("POST", update, &moderator, Some(subject))
            .await
            .0,
        StatusCode::OK
    );
    let row = h.state.store.blob(&did, &cid).await.unwrap().unwrap();
    assert!(row.storage_key.starts_with("tmp/"), "{row:?}");
    let (st, _, body) = h
        .raw_get(format!(
            "/xrpc/com.atproto.sync.getBlob?did={did}&cid={cid}"
        ))
        .await;
    assert_eq!((st, body.as_slice()), (StatusCode::OK, &b"some bytes"[..]));

    // Credentials need admin.
    let pw = json!({"did": did, "password": "new-password"});
    assert_eq!(
        h.admin_call(
            "POST",
            "/xrpc/com.atproto.admin.updateAccountPassword",
            &moderator,
            Some(pw.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        h.admin_call(
            "POST",
            "/xrpc/com.atproto.admin.updateAccountPassword",
            &admin,
            Some(pw)
        )
        .await
        .0,
        StatusCode::OK
    );
    h.create_session("yara.pds.example.com", "new-password")
        .await;
    let (st, v) = h
        .admin_call(
            "POST",
            "/xrpc/com.atproto.admin.updateAccountHandle",
            &admin,
            Some(json!({"did": did, "handle": "yarrow.pds.example.com"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        h.state
            .store
            .account_by_did(&did)
            .await
            .unwrap()
            .unwrap()
            .handle,
        "yarrow.pds.example.com"
    );
    let (st, _) = h
        .admin_call(
            "POST",
            "/xrpc/com.atproto.admin.updateAccountEmail",
            &admin,
            Some(json!({"account": did, "email": "Yarrow@Example.com"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        h.state
            .store
            .account_by_did(&did)
            .await
            .unwrap()
            .unwrap()
            .email
            .as_deref(),
        Some("yarrow@example.com")
    );

    // Invites.
    let (st, v) = h
        .admin_call(
            "POST",
            "/xrpc/com.atproto.server.createInviteCodes",
            &moderator,
            Some(json!({"codeCount": 2, "useCount": 1, "forAccounts": [did]})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let code = v["codes"][0]["codes"][0].as_str().unwrap().to_owned();
    let (_, v) = h
        .admin_call(
            "GET",
            "/xrpc/com.atproto.admin.getInviteCodes?sort=recent&limit=1",
            &moderator,
            None,
        )
        .await;
    assert_eq!(v["codes"].as_array().unwrap().len(), 1);
    let (_, page2) = h
        .admin_call(
            "GET",
            &format!(
                "/xrpc/com.atproto.admin.getInviteCodes?sort=recent&limit=5&cursor={}",
                v["cursor"].as_str().unwrap()
            ),
            &moderator,
            None,
        )
        .await;
    assert_eq!(
        page2["codes"].as_array().unwrap().len(),
        1,
        "paged past the first"
    );
    assert_eq!(
        h.admin_call(
            "POST",
            "/xrpc/com.atproto.admin.disableInviteCodes",
            &moderator,
            Some(json!({"accounts": ["admin"]}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    h.admin_call(
        "POST",
        "/xrpc/com.atproto.admin.disableInviteCodes",
        &moderator,
        Some(json!({"codes": [code]})),
    )
    .await;
    assert!(
        h.state
            .store
            .invite_code(&code)
            .await
            .unwrap()
            .unwrap()
            .disabled
    );
    let (_, v) = h.admin_call("GET", &info, &viewer, None).await;
    assert_eq!(v["invites"].as_array().unwrap().len(), 2);

    // Moderation email.
    let (st, v) = h
        .admin_call(
            "POST",
            "/xrpc/com.atproto.admin.sendEmail",
            &moderator,
            Some(json!({"recipientDid": did, "content": "<p>hello</p>", "senderDid": "did:plc:x"})),
        )
        .await;
    assert_eq!(
        (st, v["sent"].as_bool()),
        (StatusCode::OK, Some(true)),
        "{v}"
    );
    assert!(h.mail.lock().unwrap().iter().any(
        |m| m.to == "yarrow@example.com" && m.sender == locus_store::email::Sender::Moderation
    ));

    // The moderation service gets the reference's mod-service access only.
    let (mod_did, mod_access) = h.user("zed.pds.example.com").await;
    h.state
        .settings
        .set("moderation.service_did", json!(mod_did), "test")
        .await
        .unwrap();
    let (_, t) = h
        .service_token(
            &mod_access,
            &h.state.service_did,
            "com.atproto.admin.getSubjectStatus",
        )
        .await;
    let svc = format!("Bearer {}", t["token"].as_str().unwrap());
    assert_eq!(
        h.admin_call("GET", &status, &svc, None).await.0,
        StatusCode::OK
    );
    let (_, t) = h
        .service_token(
            &mod_access,
            &h.state.service_did,
            "com.atproto.admin.deleteAccount",
        )
        .await;
    let svc = format!("Bearer {}", t["token"].as_str().unwrap());
    assert_eq!(
        h.admin_call(
            "POST",
            "/xrpc/com.atproto.admin.deleteAccount",
            &svc,
            Some(json!({"did": did}))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );

    // Every action is in the audit log, by who did it.
    let audit = h.state.store.audit_entries(None, 100).await.unwrap();
    assert!(
        audit
            .iter()
            .any(|e| e.action == "admin.updateSubjectStatus" && e.actor == "operator:mo")
    );
    assert!(
        audit
            .iter()
            .any(|e| e.action == "admin.updateAccountPassword" && e.actor == "operator:ad")
    );

    // Basic auth can be turned off.
    h.state
        .settings
        .set("admin.basic_auth_enabled", json!(false), "test")
        .await
        .unwrap();
    assert_eq!(
        h.admin_call("GET", &info, &h.basic_admin(), None).await.0,
        StatusCode::UNAUTHORIZED
    );
}

impl Harness {
    /// Signs in; returns the access token.
    async fn create_session(&self, identifier: &str, password: &str) -> String {
        let (st, v) = self
            .post(
                "com.atproto.server.createSession",
                None,
                json!({"identifier": identifier, "password": password}),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        v["accessJwt"].as_str().unwrap().to_owned()
    }
}

#[tokio::test]
async fn the_locus_namespace_is_gated_by_role_group() {
    let h = harness(false).await;
    let (did, _) = h.user("abe.pds.example.com").await;
    let viewer = format!("Bearer {}", h.operator("vee", "viewer").await);
    let moderator = format!("Bearer {}", h.operator("mo", "moderator").await);
    let admin = format!("Bearer {}", h.operator("ad", "admin").await);
    let owner = format!("Bearer {}", h.operator("own", "owner").await);
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);

    // Ungated, and by group.
    let (st, v) = h.get(ns("describeCapabilities")).await;
    assert_eq!((st, v["locus"].as_bool()), (StatusCode::OK, Some(true)));
    let (st, v) = h
        .admin_call("GET", &ns("viewer.getSettings"), &viewer, None)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        v["settings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["key"] == "log.level")
    );
    assert_eq!(
        h.admin_call("GET", &ns("viewer.getSettings"), &h.basic_admin(), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED,
        "the admin password doesn't reach locus methods"
    );
    let set = |level: &str| json!({"key": "log.level", "value": level});
    assert_eq!(
        h.admin_call(
            "POST",
            &ns("admin.setSetting"),
            &moderator,
            Some(set("debug"))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        h.admin_call("POST", &ns("admin.setSetting"), &admin, Some(set("loud")))
            .await
            .0,
        StatusCode::BAD_REQUEST,
        "validated"
    );
    assert_eq!(
        h.admin_call("POST", &ns("admin.setSetting"), &admin, Some(set("debug")))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(h.state.snapshot().text("log.level"), "debug");
    let gate = json!({"key": "kryphocron.private_media_gated", "value": true});
    assert_eq!(
        h.admin_call("POST", &ns("admin.setSetting"), &admin, Some(gate))
            .await
            .0,
        StatusCode::BAD_REQUEST,
        "needs a scanner"
    );
    let (_, log) = h
        .admin_call("GET", &ns("viewer.getAuditLog"), &viewer, None)
        .await;
    let change = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["action"] == "setting.change")
        .unwrap();
    assert_eq!(
        (change["actor"].as_str(), &change["detail"]["after"]),
        (Some("operator:ad"), &json!("debug"))
    );

    // The private-tier block (moderator).
    let block = json!({"did": did, "blocked": true});
    assert_eq!(
        h.admin_call(
            "POST",
            &ns("moderator.account.setPrivateTierBlock"),
            &viewer,
            Some(block.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        h.admin_call(
            "POST",
            &ns("moderator.account.setPrivateTierBlock"),
            &moderator,
            Some(block)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(h.state.store.private_tier_blocked(&did).await.unwrap());

    // A cleared review releases the blob.
    let mut tx = h.state.store.begin().await.unwrap();
    tx.insert_temp_blob(&did, "bafyheld", "image/png", 3, "tmp/held")
        .await
        .unwrap();
    tx.quarantine_match(&did, "bafyheld").await.unwrap();
    tx.commit().await.unwrap();
    let id = h
        .state
        .store
        .add_review(&did, "bafyheld", "upload", "quarantine", None)
        .await
        .unwrap();
    let (_, q) = h
        .admin_call("GET", &ns("moderator.review.listQueue"), &moderator, None)
        .await;
    assert_eq!(q["reviews"].as_array().unwrap().len(), 1);
    let (st, _) = h
        .admin_call(
            "POST",
            &ns("moderator.review.resolve"),
            &moderator,
            Some(json!({"id": id, "status": "cleared", "note": "a cat"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let row = h.state.store.blob(&did, "bafyheld").await.unwrap().unwrap();
    assert_eq!((row.state.as_str(), row.review_hold), ("temp", false));

    // Operators and tokens (owner).
    let new_op = json!({"username": "newbie", "password": "a long password!", "role": "viewer"});
    assert_eq!(
        h.admin_call(
            "POST",
            &ns("owner.createOperator"),
            &admin,
            Some(new_op.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        h.admin_call("POST", &ns("owner.createOperator"), &owner, Some(new_op))
            .await
            .0,
        StatusCode::OK
    );
    let (st, t) = h
        .admin_call(
            "POST",
            &ns("owner.createApiToken"),
            &owner,
            Some(json!({"operator": "newbie", "name": "ci"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{t}");
    let newbie = format!("Bearer {}", t["token"].as_str().unwrap());
    assert_eq!(
        h.admin_call("GET", &ns("viewer.listJobs"), &newbie, None)
            .await
            .0,
        StatusCode::OK
    );
    h.admin_call(
        "POST",
        &ns("owner.revokeApiToken"),
        &owner,
        Some(json!({"id": t["id"]})),
    )
    .await;
    assert_eq!(
        h.admin_call("GET", &ns("viewer.listJobs"), &newbie, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (st, v) = h
        .admin_call(
            "POST",
            &ns("owner.updateOperator"),
            &owner,
            Some(json!({"username": "own", "role": "admin"})),
        )
        .await;
    assert_eq!(
        (st, v["message"].as_str()),
        (
            StatusCode::BAD_REQUEST,
            Some("There must be at least one owner")
        )
    );
    let (_, ops) = h
        .admin_call("GET", &ns("owner.listOperators"), &owner, None)
        .await;
    assert_eq!(ops["operators"].as_array().unwrap().len(), 5);

    // Kryphocron views and controls.
    let (st, v) = h
        .admin_call("GET", &ns("viewer.kryphocron.getStatus"), &viewer, None)
        .await;
    assert_eq!(st, StatusCode::OK);
    let before = v["rotation"]["generation"].clone();
    let (st, v) = h
        .admin_call("POST", &ns("admin.kryphocron.rotateNow"), &admin, None)
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (_, after) = h
        .admin_call("GET", &ns("viewer.kryphocron.getStatus"), &viewer, None)
        .await;
    assert_ne!(after["rotation"]["generation"], before);
}

#[tokio::test]
async fn the_web_ui_s_assets_are_served_immutable_by_hash() {
    let h = harness(false).await;
    let href = locus_web::assets::CSS_HREF.as_str();
    let (st, headers, body) = h.raw("GET", href, None, vec![], &[]).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(headers["content-type"], "text/css; charset=utf-8");
    assert_eq!(headers["cache-control"], locus_web::assets::IMMUTABLE);
    assert_eq!(body, locus_web::assets::CSS.as_bytes());
    let (st, headers, _) = h
        .raw(
            "GET",
            "/static/fonts/atkinson-hyperlegible-1.006-400.woff2",
            None,
            vec![],
            &[],
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(headers["content-type"], "font/woff2");
    for missing in ["/static/locus-000000000000.css", "/static/fonts/nope.woff2"] {
        let (st, _, _) = h.raw("GET", missing, None, vec![], &[]).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{missing}");
    }
}

#[tokio::test]
async fn setup_mode_serves_only_the_wizard_until_it_finishes() {
    let h = setup_harness("pds.example.com", true).await;
    assert!(
        h.state
            .store
            .create_setup_token(&crate::setup::token_hash("the-token"))
            .await
            .unwrap()
    );
    assert!(
        !h.state
            .store
            .create_setup_token(&crate::setup::token_hash("other"))
            .await
            .unwrap(),
        "first node wins"
    );

    // Only the wizard, health and /tls-check answer.
    let (st, v) = h
        .call("GET", "/xrpc/com.atproto.server.describeServer", None, None)
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("ServiceUnavailable"))
    );
    assert_eq!(
        h.call("GET", "/xrpc/_health", None, None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        h.call("GET", "/readyz", None, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        h.call("GET", "/admin", None, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        h.call("GET", "/tls-check?domain=pds.example.com", None, None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        h.call("GET", "/tls-check?domain=alice.example.com", None, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (st, headers, _) = h.raw("GET", "/", None, vec![], &[]).await;
    assert_eq!(
        (st, headers["location"].to_str().unwrap()),
        (StatusCode::SEE_OTHER, "/setup")
    );

    // The token comes first.
    let b = Browser::default();
    let (st, _, page) = b.get(&h, "/setup").await;
    assert!(
        st == StatusCode::OK && page.contains("setup token"),
        "{page}"
    );
    let (st, _, _) = b.get(&h, "/setup/hostname").await;
    assert_eq!(st, StatusCode::SEE_OTHER, "no session, no wizard");
    let (st, _, _) = b.post(&h, "/setup/token", &[("token", "wrong")]).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, headers, _) = b.post(&h, "/setup/token", &[("token", "the-token")]).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let set = headers["set-cookie"].to_str().unwrap();
    assert!(
        set.contains("HttpOnly") && set.contains("SameSite=Strict") && set.contains("Secure"),
        "{set}"
    );

    let (st, _, page) = b.get(&h, "/setup/hostname").await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        page.contains(r#"value="pds.example.com""#),
        "seeded from the bootstrap hostname"
    );
    let csrf = hidden(&page, "csrf");
    let step = |path: &'static str, form: Vec<(&'static str, String)>| {
        let (b, h, csrf) = (&b, &h, csrf.clone());
        async move {
            let mut f: Vec<(&str, &str)> = vec![("csrf", csrf.as_str())];
            f.extend(form.iter().map(|(k, v)| (*k, v.as_str())));
            b.post(h, path, &f).await
        }
    };
    let next = |r: &(StatusCode, axum::http::HeaderMap, String)| {
        assert_eq!(r.0, StatusCode::SEE_OTHER, "{}", r.2);
        r.1["location"].to_str().unwrap().to_owned()
    };

    // A forged post is refused.
    let (st, _, _) = b
        .post(
            &h,
            "/setup/hostname",
            &[("csrf", "nope"), ("hostname", "evil.example")],
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let r = step(
        "/setup/hostname",
        vec![("hostname", "https://pds.example.com".into())],
    )
    .await;
    assert!(
        r.0 == StatusCode::BAD_REQUEST && r.2.contains("no https://"),
        "{}",
        r.2
    );
    let r = step(
        "/setup/hostname",
        vec![
            ("hostname", "PDS.example.com".into()),
            ("service_name", "Example PDS".into()),
        ],
    )
    .await;
    assert_eq!(next(&r), "/setup/storage");
    let (_, _, page) = b.get(&h, "/setup/storage").await;
    assert!(page.contains("Tested:"), "{page}");
    assert_eq!(next(&step("/setup/storage", vec![]).await), "/setup/proxy");
    assert_eq!(
        next(&step("/setup/proxy", vec![("proxy", "caddy".into())]).await),
        "/setup/handles"
    );
    let (_, _, page) = b.get(&h, "/setup/handles").await;
    assert!(
        page.contains(r#"value=".example.com" checked"#),
        "the parent wildcard is suggested and chosen: {page}"
    );
    let r = step(
        "/setup/handles",
        vec![
            ("domain", ".example.com".into()),
            ("existing_subdomains", "www\nmail".into()),
        ],
    )
    .await;
    assert_eq!(next(&r), "/setup/operator");
    let (_, _, page) = b.get(&h, "/setup/handles").await;
    assert!(
        page.contains("<code>*.example.com</code>")
            && page.contains("<code>www.example.com</code>"),
        "{page}"
    );

    let r = step(
        "/setup/operator",
        vec![
            ("username", "alice".into()),
            ("password", "correct horse battery".into()),
            ("password2", "different".into()),
        ],
    )
    .await;
    assert_eq!(r.0, StatusCode::BAD_REQUEST);
    let r = step(
        "/setup/operator",
        vec![
            ("username", "alice".into()),
            ("password", "correct horse battery".into()),
            ("password2", "correct horse battery".into()),
        ],
    )
    .await;
    assert_eq!(next(&r), "/setup/email");
    let draft = h
        .state
        .store
        .setup_state()
        .await
        .unwrap()
        .unwrap()
        .draft_json;
    assert!(
        !draft.contains("correct horse") && draft.contains("$argon2id$"),
        "only a hash is kept: {draft}"
    );

    // A test email, its code, then on.
    let r = step(
        "/setup/email",
        vec![
            ("do", "save".into()),
            ("from_address", "PDS <pds@example.com>".into()),
        ],
    )
    .await;
    assert_eq!(r.0, StatusCode::OK, "{}", r.2);
    let r = step(
        "/setup/email",
        vec![("do", "send".into()), ("to", "me@example.com".into())],
    )
    .await;
    assert!(r.2.contains("Sent to me@example.com"), "{}", r.2);
    let sent = h.mail.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        (sent.to.as_str(), sent.email.subject.as_str()),
        ("me@example.com", "Test email")
    );
    let code = sent
        .email
        .text
        .split_whitespace()
        .find(|w| w.len() == 11 && w.as_bytes()[5] == b'-')
        .unwrap()
        .to_owned();
    let r = step(
        "/setup/email",
        vec![("do", "verify".into()), ("code", "AAAAA-AAAAA".into())],
    )
    .await;
    assert_eq!(r.0, StatusCode::BAD_REQUEST);
    let r = step(
        "/setup/email",
        vec![("do", "verify".into()), ("code", code.to_lowercase())],
    )
    .await;
    assert!(r.2.contains("email works"), "{}", r.2);
    assert_eq!(
        next(&step("/setup/email", vec![("do", "next".into())]).await),
        "/setup/services"
    );

    let svc = vec![
        ("appview_url", "https://api.bsky.app".into()),
        ("appview_did", "did:web:api.bsky.app".into()),
        ("crawlers", "https://bsky.network".into()),
        ("moderation_url", "https://mod.example.com".into()),
        ("moderation_did", "did:web:mod.example.com".into()),
        ("report_url", "https://mod.bsky.app".into()),
        ("report_did", "did:plc:ar7c4by46qjdydhdevvrndac".into()),
    ];
    assert_eq!(next(&step("/setup/services", svc).await), "/setup/private");
    // A private AppView needs its DID too.
    let (st, _, page) = step(
        "/setup/private",
        vec![
            ("private_tier", "1".into()),
            ("private_appview_url", "https://prism.example.com".into()),
        ],
    )
    .await;
    assert_ne!(st, StatusCode::SEE_OTHER);
    assert!(
        page.contains("The private AppView needs both its URL and its DID."),
        "{page}"
    );
    assert_eq!(
        next(
            &step(
                "/setup/private",
                vec![
                    ("private_tier", "1".into()),
                    ("private_appview_url", "https://prism.example.com".into()),
                    ("private_appview_did", "did:web:prism.example.com".into()),
                ]
            )
            .await
        ),
        "/setup/keys"
    );
    let (_, _, page) = b.get(&h, "/setup/keys").await;
    assert!(
        page.contains(&h.state.secrets.plc_keys.current_did().unwrap()),
        "the rotation key is shown"
    );
    // The key backup downloads from this step.
    let r = step(
        "/setup/keys/backup",
        vec![
            ("passphrase", "a setup passphrase".into()),
            ("passphrase2", "a different one!".into()),
        ],
    )
    .await;
    assert_eq!(r.0, StatusCode::BAD_REQUEST, "the two must match");
    let r = step(
        "/setup/keys/backup",
        vec![
            ("passphrase", "a setup passphrase".into()),
            ("passphrase2", "a setup passphrase".into()),
        ],
    )
    .await;
    assert_eq!(r.0, StatusCode::OK);
    assert!(
        r.1["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment; filename=\"aurora-locus-keys-")
    );
    assert!(r.2.starts_with("{\"format\":\"aurora-locus-key-backup\""));
    assert_eq!(
        h.state
            .store
            .key_state()
            .await
            .unwrap()
            .last_backup_by
            .as_deref(),
        Some("setup")
    );
    let r = step("/setup/keys", vec![]).await;
    assert!(
        r.0 == StatusCode::BAD_REQUEST && r.2.contains("master key"),
        "{}",
        r.2
    );
    assert_eq!(
        next(&step("/setup/keys", vec![("keys_ack", "1".into())]).await),
        "/setup/review"
    );

    // Review: warnings (no DNS or HTTPS in tests) must be accepted first.
    let (_, _, page) = b.get(&h, "/setup/review").await;
    assert!(page.contains("Example PDS") || page.contains("pds.example.com"));
    let acks: Vec<String> = page
        .match_indices(r#"name="ack" value=""#)
        .map(|(i, m)| page[i + m.len()..].split('"').next().unwrap().to_owned())
        .collect();
    assert!(acks.contains(&"reachable".to_owned()), "{page}");
    let r = step("/setup/review", vec![("do", "finish".into())]).await;
    assert!(
        r.0 == StatusCode::BAD_REQUEST && r.2.contains("accept each warning"),
        "{}",
        r.2
    );
    assert!(!h.state.store.setup_completed().await.unwrap());
    let mut form: Vec<(&str, String)> = vec![("do", "finish".into())];
    form.extend(acks.iter().map(|a| ("ack", a.clone())));
    let r = step("/setup/review", form).await;
    assert!(
        r.0 == StatusCode::OK && r.2.contains("Setup is complete"),
        "{}",
        r.2
    );
    assert!(
        h.state.setup.completed.is_cancelled(),
        "the node restarts into service"
    );

    // Everything landed.
    let row = h.state.store.setup_state().await.unwrap().unwrap();
    assert_eq!(
        (row.hostname.as_deref(), row.completed_by.as_deref()),
        (Some("pds.example.com"), Some("setup"))
    );
    let snap = h.state.store.load_snapshot().await.unwrap();
    assert_eq!(snap.list("identity.handle_domains"), [".example.com"]);
    assert_eq!(
        snap.list("identity.reserved_handles"),
        ["mail", "pds", "www"]
    );
    assert_eq!(
        snap.text("moderation.service_did"),
        "did:web:mod.example.com"
    );
    assert_eq!(snap.text("server.service_name"), "Example PDS");
    assert_eq!(snap.text("email.from_address"), "PDS <pds@example.com>");
    assert!(snap.bool("kryphocron.private_tier_enabled"));
    assert_eq!(
        snap.text("appview.private_url"),
        "https://prism.example.com"
    );
    assert_eq!(
        snap.text("appview.private_did"),
        "did:web:prism.example.com"
    );
    let op = h.state.store.operator("alice").await.unwrap().unwrap();
    assert_eq!(op.role, "owner");
    assert!(locus_auth::password::verify(
        "correct horse battery",
        &op.password_hash
    ));
    let audit = h.state.store.audit_entries(None, 100).await.unwrap();
    assert!(
        audit
            .iter()
            .any(|e| e.action == "setup.complete" && e.actor == "setup")
    );

    // The token and session are done; a second finish changes nothing.
    assert!(
        !h.state
            .store
            .setup_token_matches(&crate::setup::token_hash("the-token"))
            .await
            .unwrap()
    );
    let (_, _, page) = b.get(&h, "/setup/review").await;
    assert!(page.contains("Setup is complete"));
    let mut again = crate::setup::seed_plan(&snap, Some("pds.example.com"));
    again.operator.username = "mallory".into();
    again.operator.password = Some("another long password".into());
    again.keys_backup_acknowledged = true;
    again.email.skipped = true;
    assert!(matches!(
        crate::setup::apply(&h.state.store, &h.state.master_key, &again, false).await,
        Err(crate::setup::ApplyError::AlreadyComplete)
    ));
    assert!(
        h.state.store.operator("mallory").await.unwrap().is_none(),
        "the loser wrote nothing"
    );
}

#[tokio::test]
async fn headless_setup_builds_its_plan_from_env_and_one_run_wins() {
    let h = setup_harness("pds.example.com", false).await;
    h.state
        .store
        .create_setup_token(&crate::setup::token_hash("t"))
        .await
        .unwrap();
    let env: std::collections::HashMap<String, String> = [
        ("PDS_HOSTNAME", "pds.example.com"),
        ("PDS_ADMIN_PASSWORD", "from-the-reference-env"),
        ("LOCUS_SETUP_PROXY", "other"),
        ("LOCUS_SETUP_KEYS_BACKUP_ACKNOWLEDGED", "true"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let plan = crate::setup::plan_from_env(
        crate::setup::seed_plan(&h.state.snapshot(), Some("pds.example.com")),
        &env,
    );
    assert!(plan.email.skipped, "no SMTP URL, no email");
    assert!(!plan.has_operator() && plan.admin_password.is_some());
    assert_eq!(plan.handle_domains, [".example.com"]);

    // Without the backup acknowledgement nothing is applied.
    let mut unacked = env.clone();
    unacked.remove("LOCUS_SETUP_KEYS_BACKUP_ACKNOWLEDGED");
    let err = crate::setup::headless(&h.state, &unacked)
        .await
        .unwrap_err();
    assert!(err.contains("master key"), "{err}");
    assert!(!h.state.store.setup_completed().await.unwrap());

    // Two runs at once: exactly one applies.
    let (a, b) = tokio::join!(
        crate::setup::apply(&h.state.store, &h.state.master_key, &plan, false),
        crate::setup::apply(&h.state.store, &h.state.master_key, &plan, false),
    );
    let wins = [&a, &b].iter().filter(|r| r.is_ok()).count();
    assert_eq!(wins, 1, "{a:?} {b:?}");
    assert!(
        [&a, &b]
            .iter()
            .any(|r| matches!(r, Err(crate::setup::ApplyError::AlreadyComplete)))
    );
    let stored = h
        .state
        .store
        .get_secret(
            crate::state::secret_names::ADMIN_PASSWORD,
            &h.state.master_key,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&stored[..], b"from-the-reference-env");
    let snap = h.state.store.load_snapshot().await.unwrap();
    assert_eq!(snap.text("deploy.proxy_mode"), "other");
    assert_eq!(snap.list("identity.reserved_handles"), ["pds"]);
}

#[tokio::test]
async fn reserved_handles_from_setup_are_refused() {
    let h = harness(false).await;
    h.state
        .store
        .set_setting("identity.handle_domains", json!([".example.com"]), "test")
        .await
        .unwrap();
    h.state
        .store
        .set_setting(
            "identity.reserved_handles",
            json!(["kitchen", "orchard"]),
            "test",
        )
        .await
        .unwrap();
    h.state.settings.refresh().await.unwrap();
    for handle in ["kitchen.example.com", "Orchard.example.com"] {
        assert!(
            !locus_identity::handle::is_reserved(&handle.to_ascii_lowercase()[..7]),
            "not on the built-in list"
        );
        let (st, v) = h
            .call(
                "POST",
                "/xrpc/com.atproto.server.createAccount",
                None,
                Some(json!({"handle": handle, "email": "x@example.com", "password": "pw"})),
            )
            .await;
        assert_eq!(
            (st, v["error"].as_str()),
            (StatusCode::BAD_REQUEST, Some("HandleNotAvailable")),
            "{handle}: {v}"
        );
    }
    h.create_account("kitchens.example.com", "y@example.com", "pw")
        .await;
}

#[tokio::test]
async fn the_wizard_is_gone_after_setup() {
    let h = harness(false).await;
    for path in ["/setup", "/setup/hostname", "/setup/probe"] {
        assert_eq!(
            h.call("GET", path, None, None).await.0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
}

impl Harness {
    /// An operator with a real password, for the console.
    async fn console_operator(&self, name: &str, role: &str, pw: &str) {
        let hash = locus_auth::password::hash(pw).unwrap();
        assert!(
            self.state
                .store
                .create_operator(name, &hash, role)
                .await
                .unwrap()
        );
    }

    /// Signs `b` in to the console (no code app).
    async fn console_sign_in(&self, b: &Browser, name: &str, pw: &str) {
        let (_, _, page) = b.get(self, "/admin/sign-in").await;
        let csrf = hidden(&page, "csrf");
        let (st, headers, body) = b
            .post(
                self,
                "/admin/sign-in",
                &[("csrf", &csrf), ("username", name), ("password", pw)],
            )
            .await;
        assert_eq!(st, StatusCode::SEE_OTHER, "{body}");
        assert_eq!(headers["location"], "/admin");
    }
}

fn console_session(b: &Browser) -> String {
    b.jar
        .lock()
        .unwrap()
        .iter()
        .find(|(k, _)| k == "locus-console")
        .map(|(_, v)| v.clone())
        .unwrap()
}

#[tokio::test]
async fn the_console_signs_in_with_a_password_then_a_code() {
    let h = harness(false).await;
    h.console_operator("alice", "owner", "correct horse battery")
        .await;
    let b = Browser::default();
    let (st, headers, _) = b.get(&h, "/admin").await;
    assert_eq!(
        (st, headers["location"].to_str().unwrap()),
        (StatusCode::SEE_OTHER, "/admin/sign-in")
    );

    // Wrong password, forged form, then right.
    let (_, _, page) = b.get(&h, "/admin/sign-in").await;
    let csrf = hidden(&page, "csrf");
    let (st, _, _) = b
        .post(
            &h,
            "/admin/sign-in",
            &[
                ("csrf", &csrf),
                ("username", "alice"),
                ("password", "wrong"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _, _) = b
        .post(
            &h,
            "/admin/sign-in",
            &[
                ("csrf", "forged"),
                ("username", "alice"),
                ("password", "correct horse battery"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _, _) = b
        .post(
            &h,
            "/admin/sign-in",
            &[
                ("csrf", &csrf),
                ("username", "nobody"),
                ("password", "correct horse battery"),
            ],
        )
        .await;
    assert_eq!(
        st,
        StatusCode::UNAUTHORIZED,
        "an unknown operator looks like a wrong password"
    );
    h.console_sign_in(&b, "alice", "correct horse battery")
        .await;
    let set = b.cookie_header();
    assert!(set.contains("locus-console="));

    let (st, _, page) = b.get(&h, "/admin").await;
    assert_eq!(st, StatusCode::OK, "{page}");
    assert!(
        page.contains(r#"class="sidebar""#) && page.contains("Nodes running"),
        "{page}"
    );
    assert!(
        page.contains("no sign-in code app"),
        "the code app banner shows: {page}"
    );
    assert!(
        page.contains(r#"href="/admin/settings""#) && page.contains(r#"href="/admin/security""#)
    );
    let csrf = hidden(&page, "csrf");

    // The session is a credential for the admin API too.
    let id = console_session(&b);
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let (st, _) = h
        .admin_call(
            "GET",
            &ns("viewer.getSettings"),
            &format!("Console {id}"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = h
        .admin_call("GET", &ns("viewer.getSettings"), "Console 00", None)
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Appearance follows the operator.
    let (st, _, _) = b
        .post(
            &h,
            "/admin/appearance",
            &[
                ("csrf", &csrf),
                ("mode", "dark"),
                ("scheme", "teal"),
                ("font", "plex"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let (_, _, page) = b.get(&h, "/admin").await;
    assert!(
        page.contains(r#"class="mode-dark scheme-teal font-plex""#),
        "{page}"
    );

    // A code app: start, confirm with its code.
    let (st, _, _) = b
        .post(&h, "/admin/security/totp/start", &[("csrf", &csrf)])
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let (_, _, page) = b.get(&h, "/admin/security").await;
    assert!(page.contains("otpauth://totp/"), "{page}");
    // The key as a person types it from the page.
    let typed = page
        .split("<p><code>")
        .nth(1)
        .and_then(|rest| rest.split("</code>").next())
        .expect("the key is on the page");
    let typed: String = typed.chars().filter(|c| !c.is_whitespace()).collect();
    let secret = data_encoding::BASE32_NOPAD
        .decode(typed.as_bytes())
        .expect("the key is base32");
    assert_eq!(
        Some(secret.clone()),
        h.state
            .store
            .get_secret("operator/alice/totp_pending", &h.state.master_key)
            .await
            .unwrap()
            .map(|s| s.to_vec()),
        "the page shows the pending secret"
    );
    let now = locus_core::time::now_ms() / 1000;
    let (st, _, page) = b
        .post(
            &h,
            "/admin/security/totp/confirm",
            &[("csrf", &csrf), ("code", "000000")],
        )
        .await;
    assert!(
        st == StatusCode::BAD_REQUEST || page.contains("is on"),
        "a wrong code is refused (unless it happens to be right)"
    );
    // The page shows a QR code of the link.
    assert!(
        page.contains(r#"<div class="totp-qr""#) && page.contains("<svg"),
        "{page}"
    );
    // A code from a clock five minutes ahead says so.
    let (st, _, page) = b
        .post(
            &h,
            "/admin/security/totp/confirm",
            &[
                ("csrf", &csrf),
                ("code", &locus_auth::totp::code_at(&secret, now / 30 + 10)),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(page.contains("5 minutes ahead of"), "{page}");
    let code = locus_auth::totp::code_at(&secret, now / 30);
    let (st, _, page) = b
        .post(
            &h,
            "/admin/security/totp/confirm",
            &[("csrf", &csrf), ("code", &code)],
        )
        .await;
    assert!(st == StatusCode::OK && page.contains("is on"), "{page}");
    assert!(
        h.state
            .store
            .get_secret("operator/alice/totp_pending", &h.state.master_key)
            .await
            .unwrap()
            .is_none()
    );

    // Sign out; signing in now needs the code, and a code works once.
    let (st, _, _) = b.post(&h, "/admin/sign-out", &[("csrf", &csrf)]).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(b.get(&h, "/admin").await.0, StatusCode::SEE_OTHER);
    let (_, _, page) = b.get(&h, "/admin/sign-in").await;
    let pre = hidden(&page, "csrf");
    let (st, headers, _) = b
        .post(
            &h,
            "/admin/sign-in",
            &[
                ("csrf", &pre),
                ("username", "alice"),
                ("password", "correct horse battery"),
            ],
        )
        .await;
    assert_eq!(
        (st, headers["location"].to_str().unwrap()),
        (StatusCode::SEE_OTHER, "/admin/sign-in/code")
    );
    assert_eq!(
        b.get(&h, "/admin").await.0,
        StatusCode::SEE_OTHER,
        "half signed in isn't signed in"
    );
    let half = console_session(&b);
    let (st, _) = h
        .admin_call(
            "GET",
            &ns("viewer.getSettings"),
            &format!("Console {half}"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "nor for the API");
    let (_, _, page) = b.get(&h, "/admin/sign-in/code").await;
    let ccsrf = hidden(&page, "csrf");
    let (st, _, _) = b
        .post(
            &h,
            "/admin/sign-in/code",
            &[("csrf", &ccsrf), ("code", &code)],
        )
        .await;
    assert_eq!(
        st,
        StatusCode::UNAUTHORIZED,
        "the code that turned the app on was used"
    );
    let next = locus_auth::totp::code_at(&secret, now / 30 + 1);
    let (st, headers, _) = b
        .post(
            &h,
            "/admin/sign-in/code",
            &[("csrf", &ccsrf), ("code", &next)],
        )
        .await;
    assert_eq!(
        (st, headers["location"].to_str().unwrap()),
        (StatusCode::SEE_OTHER, "/admin")
    );
    assert_ne!(console_session(&b), half, "a new session id once signed in");
    let (st, _, page) = b.get(&h, "/admin").await;
    assert!(
        st == StatusCode::OK && !page.contains("no sign-in code app"),
        "{page}"
    );
    let audit = h.state.store.audit_entries(None, 50).await.unwrap();
    assert!(
        audit
            .iter()
            .any(|e| e.action == "operator.totp_on" && e.actor == "operator:alice")
    );
    assert!(
        audit
            .iter()
            .filter(|e| e.action == "console.sign_in")
            .count()
            >= 2
    );
}

#[tokio::test]
async fn console_settings_save_through_the_admin_api_by_role() {
    let h = harness(true).await;
    h.console_operator("alice", "admin", "correct horse battery")
        .await;
    h.console_operator("vic", "viewer", "another long password")
        .await;
    let b = Browser::default();
    h.console_sign_in(&b, "alice", "correct horse battery")
        .await;
    let (st, _, page) = b.get(&h, "/admin/settings?q=invite").await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        page.contains("accounts.invite_required") && !page.contains("log.level"),
        "searched"
    );
    let csrf = hidden(&page, "csrf");

    // A checkbox left unticked saves false.
    let (st, headers, _) = b
        .post(
            &h,
            "/admin/settings/save",
            &[
                ("csrf", &csrf),
                ("key", "accounts.invite_required"),
                ("back", "/admin/settings?q=invite"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert!(
        headers["location"]
            .to_str()
            .unwrap()
            .starts_with("/admin/settings?q=invite&saved=")
    );
    assert!(!h.state.snapshot().bool("accounts.invite_required"));
    let audit = h.state.store.audit_entries(None, 20).await.unwrap();
    assert!(audit.iter().any(|e| e.action == "setting.change"
        && e.actor == "operator:alice"
        && e.target.as_deref() == Some("accounts.invite_required")));

    // Lists and numbers; a bad value is refused with why.
    let (st, _, _) = b
        .post(
            &h,
            "/admin/settings/save",
            &[
                ("csrf", &csrf),
                ("key", "sync.crawlers"),
                ("value", "https://a.example\nhttps://b.example"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(
        h.state.snapshot().list("sync.crawlers"),
        ["https://a.example", "https://b.example"]
    );
    let (st, _, page) = b
        .post(
            &h,
            "/admin/settings/save",
            &[
                ("csrf", &csrf),
                ("key", "admin.session_idle_minutes"),
                ("value", "1"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{page}");
    // An unsafe back goes to the settings page instead.
    let (_, headers, _) = b
        .post(
            &h,
            "/admin/settings/save",
            &[
                ("csrf", &csrf),
                ("key", "accounts.invite_required"),
                ("value", "true"),
                ("back", "https://evil.example"),
            ],
        )
        .await;
    assert!(
        headers["location"]
            .to_str()
            .unwrap()
            .starts_with("/admin/settings?saved=")
    );

    // The portal switch.
    let (_, _, page) = b.get(&h, "/admin/portal").await;
    assert!(page.contains("Turn the portal off"), "{page}");
    let (st, _, _) = b
        .post(
            &h,
            "/admin/settings/save",
            &[
                ("csrf", &csrf),
                ("key", "portal.enabled"),
                ("back", "/admin/portal"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert!(!h.state.snapshot().bool("portal.enabled"));
    let (_, _, page) = b.get(&h, "/admin/portal?saved=1").await;
    assert!(
        page.contains("Turn the portal on") && page.contains("is off"),
        "{page}"
    );

    // A viewer reads but can't change.
    let v = Browser::default();
    h.console_sign_in(&v, "vic", "another long password").await;
    let (_, _, page) = v.get(&h, "/admin/settings").await;
    assert!(
        !page.contains(">Save</button>") && page.contains("disabled"),
        "read-only for a viewer"
    );
    let (_, _, page) = v.get(&h, "/admin/portal").await;
    assert!(!page.contains("Turn the portal"), "{page}");
    let vcsrf = hidden(&page, "csrf");
    let (st, _, page) = v
        .post(
            &h,
            "/admin/settings/save",
            &[
                ("csrf", &vcsrf),
                ("key", "portal.enabled"),
                ("value", "true"),
            ],
        )
        .await;
    assert!(
        st == StatusCode::BAD_REQUEST && page.contains("admin role"),
        "{page}"
    );
    assert!(!h.state.snapshot().bool("portal.enabled"));
    // Another operator's session doesn't carry this one's CSRF token.
    let (st, _, _) = v
        .post(
            &h,
            "/admin/settings/save",
            &[
                ("csrf", &csrf),
                ("key", "portal.enabled"),
                ("value", "true"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // A password change ends the operator's other sign-ins.
    let other = Browser::default();
    h.console_sign_in(&other, "alice", "correct horse battery")
        .await;
    let (st, _, page) = b
        .post(
            &h,
            "/admin/security/password",
            &[
                ("csrf", &csrf),
                ("current", "wrong"),
                ("new", "a brand new password"),
                ("new2", "a brand new password"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{page}");
    let (st, _, page) = b
        .post(
            &h,
            "/admin/security/password",
            &[
                ("csrf", &csrf),
                ("current", "correct horse battery"),
                ("new", "a brand new password"),
                ("new2", "a brand new password"),
            ],
        )
        .await;
    assert!(
        st == StatusCode::OK && page.contains("signed out everywhere else"),
        "{page}"
    );
    assert_eq!(other.get(&h, "/admin").await.0, StatusCode::SEE_OTHER);
    assert_eq!(b.get(&h, "/admin").await.0, StatusCode::OK);

    // Re-confirming goes back where it was asked from.
    let (st, _, page) = b
        .get(&h, "/admin/confirm-identity?next=/admin/settings")
        .await;
    assert!(st == StatusCode::OK && page.contains("password"), "{page}");
    let (st, headers, _) = b
        .post(
            &h,
            "/admin/confirm-identity",
            &[
                ("csrf", &csrf),
                ("next", "/admin/settings"),
                ("password", "a brand new password"),
            ],
        )
        .await;
    assert_eq!(
        (st, headers["location"].to_str().unwrap()),
        (StatusCode::SEE_OTHER, "/admin/settings")
    );
    let (_, headers, _) = b
        .post(
            &h,
            "/admin/confirm-identity",
            &[
                ("csrf", &csrf),
                ("next", "//evil.example"),
                ("password", "a brand new password"),
            ],
        )
        .await;
    assert_eq!(headers["location"], "/admin");
}

/// Makes every console session's last confirmation old.
async fn age_console_confirmations(h: &Harness) {
    let q = "UPDATE operator_session SET confirmed_at = 0";
    match &h.state.store {
        Store::Sqlite(p) => sqlx::query(q).execute(p).await.map(|_| ()),
        Store::Postgres(p) => sqlx::query(q).execute(p).await.map(|_| ()),
    }
    .unwrap();
}

#[tokio::test]
async fn console_people_pages_act_through_the_admin_api() {
    let h = harness(true).await;
    h.console_operator("alice", "owner", "correct horse battery")
        .await;
    h.console_operator("mo", "moderator", "a moderator password")
        .await;
    let b = Browser::default();
    h.console_sign_in(&b, "alice", "correct horse battery")
        .await;
    let (_, _, page) = b.get(&h, "/admin/accounts").await;
    assert!(
        page.contains("No accounts yet") && page.contains("Create an account"),
        "{page}"
    );
    let csrf = hidden(&page, "csrf");

    // Create, with an invite the console makes (sign-ups need one here).
    let (st, headers, body) = b
        .post(
            &h,
            "/admin/accounts/create",
            &[
                ("csrf", &csrf),
                ("handle", "carol.pds.example.com"),
                ("email", "carol@example.com"),
                ("password", "carols password"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{body}");
    let href = headers["location"]
        .to_str()
        .unwrap()
        .split('?')
        .next()
        .unwrap()
        .to_owned();
    let carol = h
        .state
        .store
        .account_by_handle("carol.pds.example.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(href, format!("/admin/accounts/{}", carol.did));
    assert!(carol.did.starts_with("did:plc:"), "{}", carol.did);
    // did:web on request, with its warnings shown first.
    let (_, _, page) = b.get(&h, "/admin/accounts").await;
    assert!(
        page.contains(r#"name="did_method" value="web""#)
            && page.contains("There are no rotation keys"),
        "{page}"
    );
    let (st, _, body) = b
        .post(
            &h,
            "/admin/accounts/create",
            &[
                ("csrf", &csrf),
                ("handle", "dot.pds.example.com"),
                ("email", "dot@example.com"),
                ("password", "dots long password"),
                ("did_method", "web"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{body}");
    let dot = h
        .state
        .store
        .account_by_handle("dot.pds.example.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(dot.did, "did:web:dot.pds.example.com");
    assert!(
        h.state
            .store
            .hosted_did_web(&dot.did)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        h.state
            .store
            .invited_by(&carol.did)
            .await
            .unwrap()
            .is_some(),
        "an invite was made and used"
    );
    let (_, _, page) = b.get(&h, "/admin/accounts?q=carol").await;
    assert!(page.contains("@carol.pds.example.com"), "{page}");
    let (st, _, page) = b.get(&h, &href).await;
    assert!(
        st == StatusCode::OK && page.contains("Take down") && page.contains("Reset password"),
        "{page}"
    );
    assert!(
        page.contains(r#"href="/admin/accounts" aria-current="page""#)
            || page.contains("aria-current=\"page\""),
        "Accounts is current"
    );

    // Takedown and its reversal, by the API, audited as the operator.
    let (st, _, _) = b
        .post(
            &h,
            &format!("{href}/takedown"),
            &[("csrf", &csrf), ("applied", "true"), ("ref", "case-1")],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let a = h
        .state
        .store
        .account_by_did(&carol.did)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (a.status, a.status_ref.as_deref()),
        (locus_store::accounts::Status::Takendown, Some("case-1"))
    );
    let audit = h.state.store.audit_entries(None, 50).await.unwrap();
    assert!(
        audit
            .iter()
            .any(|e| e.action == "admin.updateSubjectStatus" && e.actor == "operator:alice")
    );
    let (_, _, page) = b.get(&h, &href).await;
    assert!(
        page.contains("Reverse the takedown") && page.contains("case-1"),
        "{page}"
    );
    b.post(
        &h,
        &format!("{href}/takedown"),
        &[("csrf", &csrf), ("applied", "false")],
    )
    .await;
    assert_eq!(
        h.state
            .store
            .account_by_did(&carol.did)
            .await
            .unwrap()
            .unwrap()
            .status,
        locus_store::accounts::Status::Active
    );
    b.post(
        &h,
        &format!("{href}/private-block"),
        &[("csrf", &csrf), ("blocked", "true")],
    )
    .await;
    assert!(
        h.state
            .store
            .private_tier_blocked(&carol.did)
            .await
            .unwrap()
    );
    let (st, _, _) = b
        .post(
            &h,
            &format!("{href}/handle"),
            &[("csrf", &csrf), ("handle", "carla.pds.example.com")],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(
        h.state
            .store
            .account_by_did(&carol.did)
            .await
            .unwrap()
            .unwrap()
            .handle,
        "carla.pds.example.com"
    );

    // A moderator can take down but not change credentials.
    let m = Browser::default();
    h.console_sign_in(&m, "mo", "a moderator password").await;
    let (_, _, page) = m.get(&h, &href).await;
    assert!(
        page.contains("Take down") && !page.contains("Reset password"),
        "{page}"
    );
    assert!(!page.contains(r#"href="/admin/operators""#), "owners only");
    let mcsrf = hidden(&page, "csrf");
    let (st, _, page) = m
        .post(
            &h,
            &format!("{href}/password"),
            &[("csrf", &mcsrf), ("password", "stolen password")],
        )
        .await;
    assert!(
        st == StatusCode::BAD_REQUEST && page.contains("admin role"),
        "{page}"
    );
    assert_eq!(m.get(&h, "/admin/operators").await.0, StatusCode::FORBIDDEN);
    let (st, _, _) = m
        .post(
            &h,
            "/admin/operators/create",
            &[
                ("csrf", &mcsrf),
                ("username", "evil"),
                ("password", "long enough password"),
                ("role", "owner"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(h.state.store.operator("evil").await.unwrap().is_none());

    // Invites: create, see, disable.
    let (st, _, page) = b
        .post(
            &h,
            "/admin/invites/create",
            &[("csrf", &csrf), ("count", "2"), ("uses", "3")],
        )
        .await;
    assert!(st == StatusCode::OK && page.contains("New codes"), "{page}");
    let code = page
        .split("<p><code>")
        .nth(1)
        .unwrap()
        .split('<')
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(
        h.state
            .store
            .invite_code(&code)
            .await
            .unwrap()
            .unwrap()
            .available,
        3
    );
    let (st, _, _) = b
        .post(
            &h,
            "/admin/invites/disable",
            &[("csrf", &csrf), ("code", &code)],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert!(
        h.state
            .store
            .invite_code(&code)
            .await
            .unwrap()
            .unwrap()
            .disabled
    );
    // A one-use code, used once: none left, shown used up, no Disable.
    let mut tx = h.state.store.begin().await.unwrap();
    tx.insert_invite_code("used-once-code", 1, "admin", "admin")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (st, v) = h
        .post(
            "com.atproto.server.createAccount",
            None,
            json!({"handle": "uno.pds.example.com", "email": "uno@example.com",
                "password": "long enough password", "inviteCode": "used-once-code"}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (_, _, page) = b.get(&h, "/admin/invites").await;
    let row = page
        .split("<tr>")
        .find(|r| r.contains("used-once-code"))
        .expect("the code is listed");
    assert!(
        row.contains(r#"<td class="num">0</td><td class="num">1</td>"#),
        "{row}"
    );
    assert!(row.contains("used up") && !row.contains("Disable"), "{row}");
    let (st, _, page) = b.get(&h, "/admin/operators").await;
    assert!(st == StatusCode::OK && page.contains("mo"), "{page}");
    let (st, _, page) = b
        .post(
            &h,
            "/admin/operators/token",
            &[
                ("csrf", &csrf),
                ("operator", "mo"),
                ("name", "script"),
                ("days", "30"),
            ],
        )
        .await;
    assert!(
        st == StatusCode::OK && page.contains("lop_"),
        "the token is shown once: {page}"
    );
    let (st, _, _) = b
        .post(
            &h,
            "/admin/operators/disable",
            &[("csrf", &csrf), ("username", "mo"), ("disabled", "true")],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(
        m.get(&h, "/admin").await.0,
        StatusCode::SEE_OTHER,
        "a disabled operator is signed out"
    );
    let (_, _, page) = b
        .post(
            &h,
            "/admin/operators/disable",
            &[("csrf", &csrf), ("username", "alice"), ("disabled", "true")],
        )
        .await;
    assert!(page.contains("disable yourself"), "{page}");
    age_console_confirmations(&h).await;
    let (st, headers, _) = b.get(&h, "/admin/operators").await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert!(
        headers["location"]
            .to_str()
            .unwrap()
            .starts_with("/admin/confirm-identity?next=%2Fadmin%2Foperators")
    );
    let (st, headers, _) = b
        .post(
            &h,
            "/admin/operators/create",
            &[
                ("csrf", &csrf),
                ("username", "zed"),
                ("password", "long enough password"),
                ("role", "viewer"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER, "an action needs it too");
    assert!(
        headers["location"]
            .to_str()
            .unwrap()
            .starts_with("/admin/confirm-identity")
    );
    assert!(h.state.store.operator("zed").await.unwrap().is_none());
    b.post(
        &h,
        "/admin/confirm-identity",
        &[
            ("csrf", &csrf),
            ("next", "/admin/operators"),
            ("password", "correct horse battery"),
        ],
    )
    .await;
    let (st, _, _) = b
        .post(
            &h,
            "/admin/operators/create",
            &[
                ("csrf", &csrf),
                ("username", "zed"),
                ("password", "long enough password"),
                ("role", "viewer"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(
        h.state.store.operator("zed").await.unwrap().unwrap().role,
        "viewer"
    );

    // Deleting an account: the typed word, freshly confirmed.
    let (st, _, page) = b.get(&h, &format!("{href}/delete")).await;
    assert!(
        st == StatusCode::OK && page.contains("<code>delete</code>"),
        "{page}"
    );
    let (st, _, _) = b
        .post(
            &h,
            &format!("{href}/delete"),
            &[("csrf", &csrf), ("typed", "nope")],
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, headers, _) = b
        .post(
            &h,
            &format!("{href}/delete"),
            &[("csrf", &csrf), ("typed", "delete")],
        )
        .await;
    assert_eq!(
        (st, headers["location"].to_str().unwrap()),
        (StatusCode::SEE_OTHER, "/admin/accounts?done=deleted")
    );
    for _ in 0..50 {
        if h.state
            .store
            .account_by_did(&carol.did)
            .await
            .unwrap()
            .is_none_or(|a| a.status == locus_store::accounts::Status::Deleted)
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        h.state
            .store
            .account_by_did(&carol.did)
            .await
            .unwrap()
            .is_none_or(|a| a.status == locus_store::accounts::Status::Deleted)
    );
}

#[tokio::test]
async fn every_console_page_renders_for_each_role_under_the_csp() {
    let h = harness(false).await;
    h.console_operator("own", "owner", "an owner password")
        .await;
    h.console_operator("vee", "viewer", "a viewer password!")
        .await;
    let (did, _) = h.user("dana.pds.example.com").await;
    let o = Browser::default();
    h.console_sign_in(&o, "own", "an owner password").await;
    let v = Browser::default();
    h.console_sign_in(&v, "vee", "a viewer password!").await;
    for page in locus_web::console::PAGES.iter().filter(|p| p.id.built()) {
        for (who, b, role) in [("owner", &o, "owner"), ("viewer", &v, "viewer")] {
            let cookie = b.cookie_header();
            let (st, headers, body) = h
                .raw("GET", page.href, None, vec![], &[("cookie", &cookie)])
                .await;
            let body = String::from_utf8(body).unwrap();
            let need = page.need;
            let allowed = locus_web::console::Need::from_role(role) >= need;
            if !allowed {
                assert_eq!(st, StatusCode::FORBIDDEN, "{who} {}", page.href);
                continue;
            }
            assert_eq!(st, StatusCode::OK, "{who} {}: {body}", page.href);
            // The base policy; the branding editor adds frame-src 'self' for
            // its previews, and live status pages their one script.
            let csp = headers["content-security-policy"].to_str().unwrap();
            assert!(
                csp == locus_web::assets::CSP
                    || csp == format!("{}; frame-src 'self'", locus_web::assets::CSP)
                    || (page.id.live() && csp == locus_web::assets::CSP_LIVE),
                "{}: {csp}",
                page.href
            );
            assert!(
                body.contains(&format!("<title>{} · ", page.id.title())),
                "{}: {body}",
                page.href
            );
            assert!(
                body.contains(&format!(r#"href="{}" aria-current="page""#, page.href)),
                "{} is marked current",
                page.href
            );
        }
    }

    // Moderation: an account taken down from the lookup.
    let (_, _, page) = o.get(&h, &format!("/admin/moderation?subject={did}")).await;
    assert!(page.contains("Take it down"), "{page}");
    let csrf = hidden(&page, "csrf");
    let (st, _, _) = o
        .post(
            &h,
            "/admin/moderation/status",
            &[
                ("csrf", &csrf),
                ("subject", &did),
                ("blob", ""),
                ("applied", "true"),
                ("ref", "mod-7"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(
        h.state
            .store
            .account_by_did(&did)
            .await
            .unwrap()
            .unwrap()
            .status,
        locus_store::accounts::Status::Takendown
    );
    let (_, _, page) = o.get(&h, &format!("/admin/moderation?subject={did}")).await;
    assert!(
        page.contains("Reverse the takedown") && page.contains("mod-7"),
        "{page}"
    );
    let vcsrf = hidden(&v.get(&h, "/admin").await.2, "csrf");
    let (st, _, _) = v
        .post(
            &h,
            "/admin/moderation/status",
            &[
                ("csrf", &vcsrf),
                ("subject", &did),
                ("blob", ""),
                ("applied", "false"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "a viewer can't");

    // Email: a test email's code confirms delivery.
    let (st, _, page) = o
        .post(
            &h,
            "/admin/email/test",
            &[("csrf", &csrf), ("to", "ops@example.com")],
        )
        .await;
    assert!(
        st == StatusCode::OK && page.contains("Sent to ops@example.com"),
        "{page}"
    );
    let sent = h.mail.lock().unwrap().last().cloned().unwrap();
    let code = sent
        .email
        .text
        .split_whitespace()
        .find(|w| w.len() == 11 && w.as_bytes()[5] == b'-')
        .unwrap()
        .to_owned();
    let (st, _, _) = o
        .post(
            &h,
            "/admin/email/verify",
            &[("csrf", &csrf), ("code", "WRONG-CODE0")],
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _, page) = o
        .post(
            &h,
            "/admin/email/verify",
            &[("csrf", &csrf), ("code", &code)],
        )
        .await;
    assert!(
        st == StatusCode::OK && page.contains("email works"),
        "{page}"
    );
    let (st, _, _) = v
        .post(
            &h,
            "/admin/email/test",
            &[("csrf", &vcsrf), ("to", "x@example.com")],
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Logs show this node's lines; the audit log filters and exports.
    h.state.logs.push("a line the test wrote".into());
    let (_, _, page) = o.get(&h, "/admin/logs").await;
    assert!(page.contains("a line the test wrote"), "{page}");
    let (_, _, page) = o.get(&h, "/admin/audit?action=updateSubjectStatus").await;
    assert!(
        page.contains("operator:own") && !page.contains("email.verified</td>"),
        "{page}"
    );
    let cookie = o.cookie_header();
    let (st, headers, body) = h
        .raw(
            "GET",
            "/admin/audit/export",
            None,
            vec![],
            &[("cookie", &cookie)],
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(headers["content-type"], "application/x-ndjson");
    let lines: Vec<Value> = String::from_utf8(body)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(lines.iter().any(|l| l["action"] == "email.verified"));
    assert_eq!(
        h.raw("GET", "/admin/audit/export", None, vec![], &[])
            .await
            .0,
        StatusCode::SEE_OTHER,
        "signed out, no export"
    );
}

/// The last emailed code (`XXXXX-XXXXX`).
fn last_code(h: &Harness) -> String {
    let sent = h
        .mail
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("an email was sent");
    sent.email
        .text
        .split_whitespace()
        .find(|w| w.len() == 11 && w.as_bytes()[5] == b'-')
        .expect("a code in the email")
        .to_owned()
}

async fn portal_sign_in(
    h: &Harness,
    b: &Browser,
    id: &str,
    pw: &str,
    code: Option<&str>,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let (_, _, page) = b.get(h, "/account/sign-in").await;
    let csrf = hidden(&page, "csrf");
    let mut form = vec![
        ("csrf", csrf.as_str()),
        ("identifier", id),
        ("password", pw),
    ];
    if let Some(c) = code {
        form.push(("code", c));
    }
    b.post(h, "/account/sign-in", &form).await
}

#[tokio::test]
async fn the_account_portal_runs_on_the_account_api() {
    let h = harness(false).await;
    let b = Browser::default();
    let (st, headers, _) = b.get(&h, "/account").await;
    assert_eq!(
        (st, headers["location"].to_str().unwrap()),
        (StatusCode::SEE_OTHER, "/account/sign-in?next=%2Faccount")
    );

    // Sign up (the handle gets this server's domain), and land signed in.
    let (_, _, page) = b.get(&h, "/account/sign-up").await;
    assert!(
        page.contains(".pds.example.com") && !page.contains("Invite code"),
        "{page}"
    );
    let csrf = hidden(&page, "csrf");
    let (st, headers, body) = b
        .post(
            &h,
            "/account/sign-up",
            &[
                ("csrf", &csrf),
                ("handle", "erin"),
                ("email", "erin@example.com"),
                ("password", "erins password"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(headers["location"], "/account");
    let erin = h
        .state
        .store
        .account_by_handle("erin.pds.example.com")
        .await
        .unwrap()
        .unwrap();
    let (st, _, page) = b.get(&h, "/account").await;
    assert!(
        st == StatusCode::OK
            && page.contains("@erin.pds.example.com")
            && page.contains("Your account"),
        "{page}"
    );
    assert!(
        page.contains("isn&#x27;t confirmed yet") || page.contains("confirmed yet"),
        "the unconfirmed-email banner"
    );
    let csrf = hidden(&page, "csrf");

    // Confirm the email with the code sent.
    let (st, _, _) = b
        .post(&h, "/account/email/send-confirm", &[("csrf", &csrf)])
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let code = last_code(&h);
    let (st, headers, _) = b
        .post(
            &h,
            "/account/email/confirm",
            &[("csrf", &csrf), ("code", &code)],
        )
        .await;
    assert_eq!(
        (st, headers["location"].to_str().unwrap()),
        (StatusCode::SEE_OTHER, "/account/email?done=confirmed")
    );
    assert!(
        h.state
            .store
            .account_by_did(&erin.did)
            .await
            .unwrap()
            .unwrap()
            .email_confirmed_at
            .is_some()
    );

    // Sign-in codes: on at once (it only adds protection, as the
    // reference); off needs a code from the inbox.
    let (st, _, _) = b
        .post(
            &h,
            "/account/email/factor",
            &[("csrf", &csrf), ("factor", "on")],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert!(
        h.state
            .store
            .account_by_did(&erin.did)
            .await
            .unwrap()
            .unwrap()
            .email_auth_factor_at
            .is_some()
    );
    let (st, _, page) = b
        .post(
            &h,
            "/account/email/factor",
            &[("csrf", &csrf), ("factor", "off")],
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "a code is needed: {page}");
    let code = last_code(&h);
    let (st, _, _) = b
        .post(
            &h,
            "/account/email/factor",
            &[("csrf", &csrf), ("factor", "off"), ("code", &code)],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert!(
        h.state
            .store
            .account_by_did(&erin.did)
            .await
            .unwrap()
            .unwrap()
            .email_auth_factor_at
            .is_none()
    );
    let (st, _, _) = b
        .post(
            &h,
            "/account/email/factor",
            &[("csrf", &csrf), ("factor", "on")],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);

    // An app password works for apps, not for the portal.
    let (st, _, page) = b
        .post(
            &h,
            "/account/app-passwords/create",
            &[("csrf", &csrf), ("name", "phone"), ("privileged", "no")],
        )
        .await;
    assert!(
        st == StatusCode::OK && page.contains("New app password: phone"),
        "{page}"
    );
    let app_pw = page
        .split("Copy it now")
        .nth(1)
        .unwrap()
        .split("<span class=\"v\">")
        .nth(1)
        .unwrap()
        .split('<')
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(app_pw.len(), 19, "{app_pw}");

    // Sign out; signing back in needs the emailed code too.
    let (st, _, _) = b.post(&h, "/account/sign-out", &[("csrf", &csrf)]).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(b.get(&h, "/account").await.0, StatusCode::SEE_OTHER);
    let (st, _, page) =
        portal_sign_in(&h, &b, "erin.pds.example.com", "erins password", None).await;
    assert!(
        st == StatusCode::OK && page.contains("emailed you a sign-in code"),
        "{page}"
    );
    let code = last_code(&h);
    // App passwords skip the code (as the reference), and are refused here.
    let (st, _, page) = portal_sign_in(&h, &b, "erin.pds.example.com", &app_pw, None).await;
    assert!(
        st == StatusCode::UNAUTHORIZED && page.contains("not an app password"),
        "{page}"
    );
    let (st, headers, body) = portal_sign_in(
        &h,
        &b,
        "erin.pds.example.com",
        "erins password",
        Some(&code),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::SEE_OTHER,
        "{}",
        body.split("role=\"alert\">").nth(1).unwrap_or(&body)
    );
    assert_eq!(headers["location"], "/account");
    let csrf = hidden(&b.get(&h, "/account").await.2, "csrf");

    // Handle, export, deactivation.
    let (st, _, _) = b
        .post(
            &h,
            "/account/handle",
            &[("csrf", &csrf), ("handle", "erin2")],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(
        h.state
            .store
            .account_by_did(&erin.did)
            .await
            .unwrap()
            .unwrap()
            .handle,
        "erin2.pds.example.com"
    );
    let cookie = b.cookie_header();
    let (st, headers, car) = h
        .raw(
            "GET",
            "/account/export/repo.car",
            None,
            vec![],
            &[("cookie", &cookie)],
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(headers["content-type"], "application/vnd.ipld.car");
    assert!(
        headers["content-disposition"]
            .to_str()
            .unwrap()
            .contains("erin2.pds.example.com.car")
    );
    assert!(!car.is_empty());
    let (st, _, page) = b.get(&h, "/account/apps").await;
    assert!(
        st == StatusCode::OK && page.contains("No app is signed in"),
        "{page}"
    );
    b.post(&h, "/account/deactivate", &[("csrf", &csrf), ("to", "off")])
        .await;
    assert_eq!(
        h.state
            .store
            .account_by_did(&erin.did)
            .await
            .unwrap()
            .unwrap()
            .status,
        locus_store::accounts::Status::Deactivated
    );
    b.post(&h, "/account/deactivate", &[("csrf", &csrf), ("to", "on")])
        .await;
    assert_eq!(
        h.state
            .store
            .account_by_did(&erin.did)
            .await
            .unwrap()
            .unwrap()
            .status,
        locus_store::accounts::Status::Active
    );

    // A password reset ends this sign-in (its session is revoked).
    b.post(&h, "/account/password/send", &[("csrf", &csrf)])
        .await;
    let code = last_code(&h);
    let (st, headers, _) = b
        .post(
            &h,
            "/account/password/reset",
            &[
                ("csrf", &csrf),
                ("code", &code),
                ("password", "a new password"),
            ],
        )
        .await;
    assert_eq!(
        (st, headers["location"].to_str().unwrap()),
        (StatusCode::SEE_OTHER, "/account/sign-in?done=password")
    );
    assert_eq!(b.get(&h, "/account").await.0, StatusCode::SEE_OTHER);
    // A stolen portal cookie dies with its session too.
    let other = Browser::default();
    let (_, _, _) =
        portal_sign_in(&h, &other, "erin2.pds.example.com", "a new password", None).await;
    let code = last_code(&h);
    let (st, _, _) = portal_sign_in(
        &h,
        &other,
        "erin2.pds.example.com",
        "a new password",
        Some(&code),
    )
    .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(other.get(&h, "/account").await.0, StatusCode::OK);
    let mut tx = h.state.store.begin().await.unwrap();
    tx.revoke_sessions(&erin.did, locus_store::accounts::SessionFilter::All)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(other.get(&h, "/account").await.0, StatusCode::SEE_OTHER);

    // The switch: off, the portal isn't there.
    h.state
        .store
        .set_setting("portal.enabled", json!(false), "test")
        .await
        .unwrap();
    h.state.settings.refresh().await.unwrap();
    for path in [
        "/account",
        "/account/sign-in",
        "/account/sign-up",
        "/account/email",
    ] {
        assert_eq!(b.get(&h, path).await.0, StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn the_portal_deletes_an_account_with_its_code() {
    let h = harness(false).await;
    h.create_account("finn.pds.example.com", "finn@example.com", "finns password")
        .await;
    let b = Browser::default();
    let (st, _, _) = portal_sign_in(&h, &b, "finn@example.com", "finns password", None).await;
    assert_eq!(st, StatusCode::SEE_OTHER, "sign-in by email works too");
    let csrf = hidden(&b.get(&h, "/account/delete").await.2, "csrf");
    b.post(&h, "/account/delete/send", &[("csrf", &csrf)]).await;
    let code = last_code(&h);
    let (st, _, page) = b
        .post(
            &h,
            "/account/delete/confirm",
            &[("csrf", &csrf), ("code", &code), ("password", "wrong")],
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{page}");
    let (st, _, page) = b
        .post(
            &h,
            "/account/delete/confirm",
            &[
                ("csrf", &csrf),
                ("code", &code),
                ("password", "finns password"),
            ],
        )
        .await;
    assert!(
        st == StatusCode::OK && page.contains("being deleted"),
        "{page}"
    );
    assert_eq!(b.get(&h, "/account").await.0, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn oauth_prompt_create_signs_a_new_account_up_and_on_to_consent() {
    const LB: &str = "http://localhost?scope=atproto+transition%3Ageneric";
    let h = harness(true).await;
    let client = DpopClient::new();
    let par = |prompt: Option<&'static str>| {
        let client = &client;
        let h = &h;
        async move {
            let (_, challenge) = pkce();
            let mut form = vec![
                ("client_id", LB.to_owned()),
                ("response_type", "code".to_owned()),
                ("scope", "atproto transition:generic".to_owned()),
                ("redirect_uri", "http://127.0.0.1/".to_owned()),
                ("code_challenge", challenge),
                ("code_challenge_method", "S256".to_owned()),
            ];
            if let Some(p) = prompt {
                form.push(("prompt", p.to_owned()));
            }
            let f: Vec<(&str, &str)> = form.iter().map(|(k, v)| (*k, v.as_str())).collect();
            let (_, _, v) = client.post(h, "/oauth/par", &f).await;
            format!(
                "/oauth/authorize?client_id={}&request_uri={}",
                url::form_urlencoded::byte_serialize(LB.as_bytes()).collect::<String>(),
                url::form_urlencoded::byte_serialize(v["request_uri"].as_str().unwrap().as_bytes())
                    .collect::<String>()
            )
        }
    };
    let b = Browser::default();
    let (st, _, html) = b.get(&h, &par(Some("create")).await).await;
    assert!(
        st == StatusCode::OK
            && html.contains("Create an account for")
            && html.contains("Invite code"),
        "{html}"
    );
    let (csrf, rid) = (hidden(&html, "csrf"), hidden(&html, "request_id"));

    // Sign-ups need an invite here.
    let (st, _, html) = b
        .post(
            &h,
            "/oauth/authorize/sign-up",
            &[
                ("csrf", &csrf),
                ("request_id", &rid),
                ("handle", "gale"),
                ("email", "gale@example.com"),
                ("password", "gales password"),
            ],
        )
        .await;
    assert!(
        st.is_client_error() && html.contains("Create an account for"),
        "{st} {html}"
    );
    let invite = {
        let code = "pds-example-com-abcde-fghij";
        let mut tx = h.state.store.begin().await.unwrap();
        tx.insert_invite_code(code, 1, "admin", "admin")
            .await
            .unwrap();
        tx.commit().await.unwrap();
        code
    };
    let (st, _, html) = b
        .post(
            &h,
            "/oauth/authorize/sign-up",
            &[
                ("csrf", &csrf),
                ("request_id", &rid),
                ("handle", "gale"),
                ("email", "gale@example.com"),
                ("password", "gales password"),
                ("invite", invite),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{html}");
    assert!(
        html.contains("@gale.pds.example.com") || html.contains("gale.pds.example.com"),
        "on to consent as the new account: {html}"
    );
    assert!(
        h.state
            .store
            .account_by_handle("gale.pds.example.com")
            .await
            .unwrap()
            .is_some()
    );

    // The sign-in page links to sign-up and back.
    let b2 = Browser::default();
    let (_, _, html) = b2.get(&h, &par(None).await).await;
    assert!(
        html.contains("Sign in to") && html.contains("/oauth/authorize/show-sign-up"),
        "{html}"
    );
    let (csrf, rid) = (hidden(&html, "csrf"), hidden(&html, "request_id"));
    let (_, _, html) = b2
        .post(
            &h,
            "/oauth/authorize/show-sign-up",
            &[("csrf", &csrf), ("request_id", &rid)],
        )
        .await;
    assert!(html.contains("Create an account for"), "{html}");
    let (_, _, html) = b2
        .post(
            &h,
            "/oauth/authorize/show-sign-in",
            &[("csrf", &csrf), ("request_id", &rid)],
        )
        .await;
    assert!(html.contains("Sign in to"), "{html}");
}

/// A multipart body: text fields, then one file.
fn multipart(fields: &[(&str, &str)], file: (&str, &str, &[u8])) -> (String, Vec<u8>) {
    let boundary = "locus-test-boundary";
    let mut body = Vec::new();
    for (k, v) in fields {
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n")
                .as_bytes(),
        );
    }
    let (name, filename, bytes) = file;
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

impl Harness {
    async fn upload_brand(
        &self,
        b: &Browser,
        csrf: &str,
        kind: &str,
        filename: &str,
        bytes: &[u8],
    ) -> (StatusCode, axum::http::HeaderMap, String) {
        let (ct, body) = multipart(&[("csrf", csrf), ("kind", kind)], ("file", filename, bytes));
        let cookie = b.cookie_header();
        let (st, headers, out) = self
            .raw(
                "POST",
                "/admin/branding/upload",
                None,
                body,
                &[
                    ("cookie", &cookie),
                    ("content-type", &ct),
                    ("sec-fetch-site", "same-origin"),
                ],
            )
            .await;
        (st, headers, String::from_utf8_lossy(&out).into_owned())
    }
}

/// The draft form's fields, every one, with `over` set.
fn brand_form<'a>(csrf: &'a str, over: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut f: Vec<(&str, &str)> = vec![
        ("csrf", csrf),
        ("service_name", ""),
        ("logo", ""),
        ("favicon", ""),
        ("background_light", ""),
        ("background_dark", ""),
        ("light_primary", ""),
        ("light_error", ""),
        ("light_warning", ""),
        ("light_info", ""),
        ("light_success", ""),
        ("dark_primary", ""),
        ("dark_error", ""),
        ("dark_warning", ""),
        ("dark_info", ""),
        ("dark_success", ""),
        ("font", "system"),
        ("uploaded_font", ""),
        ("radius", "default"),
        ("spacing", "default"),
        ("link_home", ""),
        ("link_terms", ""),
        ("link_privacy", ""),
        ("link_support", ""),
        ("text_sign_in_heading", ""),
        ("text_sign_in_blurb", ""),
        ("text_sign_up_blurb", ""),
        ("text_portal_blurb", ""),
        ("text_footer", ""),
        ("custom_css", ""),
    ];
    for (k, v) in over {
        if let Some(e) = f.iter_mut().find(|(n, _)| n == k) {
            e.1 = v;
        }
    }
    f
}

#[tokio::test]
async fn branding_is_drafted_checked_published_and_dresses_the_right_pages() {
    let h = harness(false).await;
    h.console_operator("ada", "admin", "an admin password")
        .await;
    h.console_operator("vic", "viewer", "a viewer password!")
        .await;
    let b = Browser::default();
    h.console_sign_in(&b, "ada", "an admin password").await;
    let (st, headers, page) = b.get(&h, "/admin/branding").await;
    assert_eq!(st, StatusCode::OK, "{page}");
    assert!(
        headers["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-src 'self'")
    );
    assert!(
        page.contains("/admin/branding/preview/consent?mode=dark")
            && page.contains(r#"width="375""#),
        "{page}"
    );
    let csrf = hidden(&page, "csrf");

    // Uploads: what a file is decides what it may be.
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR rest of a png";
    let (st, _, body) = h.upload_brand(&b, &csrf, "logo", "logo.png", PNG).await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{body}");
    let (st, _, body) = h
        .upload_brand(
            &b,
            &csrf,
            "logo",
            "evil.svg",
            b"<svg onload=alert(1)></svg>",
        )
        .await;
    assert!(
        st == StatusCode::BAD_REQUEST && body.contains("isn"),
        "{body}"
    );
    let (st, _, _) = h.upload_brand(&b, &csrf, "font", "logo.png", PNG).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "a PNG isn't a font");
    let (st, _, _) = h.upload_brand(&b, "forged", "logo", "logo.png", PNG).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let logo = h
        .state
        .store
        .branding_assets()
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(
        (logo.kind.as_str(), logo.mime.as_str()),
        ("logo", "image/png")
    );
    let (st, headers, bytes) = h
        .raw("GET", &format!("/branding/{}", logo.id), None, vec![], &[])
        .await;
    assert_eq!(
        (st, headers["content-type"].to_str().unwrap()),
        (StatusCode::OK, "image/png")
    );
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert!(
        headers["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );
    assert_eq!(bytes, PNG);

    // A faint colour and a stylesheet import block publishing, not saving.
    let logo_id = logo.id.clone();
    let bad = brand_form(
        &csrf,
        &[
            ("logo", &logo_id),
            ("light_primary", "#eeeeee"),
            ("custom_css", "@import url(https://evil.example/x.css);"),
        ],
    );
    let (st, _, _) = b.post(&h, "/admin/branding/save", &bad).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let (_, _, page) = b.get(&h, "/admin/branding").await;
    assert!(
        page.contains("too faint") && page.contains("@import"),
        "{page}"
    );
    let (st, _, _) = b
        .post(&h, "/admin/branding/publish", &[("csrf", &csrf)])
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(h.state.store.branding_active().await.unwrap().is_none());

    // A good draft publishes.
    let good = brand_form(
        &csrf,
        &[
            ("logo", &logo_id),
            ("service_name", "Example Social"),
            ("light_primary", "#1f6feb"),
            ("dark_primary", "#4d8dff"),
            ("link_terms", "https://example.com/terms"),
            ("text_footer", "Run by Example & friends"),
            ("text_sign_in_heading", "Welcome back"),
            ("custom_css", ".brand-blurb { color: #333333 }"),
        ],
    );
    b.post(&h, "/admin/branding/save", &good).await;
    let (st, _, body) = b
        .post(&h, "/admin/branding/publish", &[("csrf", &csrf)])
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{body}");
    let v1 = h
        .state
        .store
        .branding_active()
        .await
        .unwrap()
        .unwrap()
        .version;
    assert!(
        h.state
            .store
            .audit_entries(None, 50)
            .await
            .unwrap()
            .iter()
            .any(|e| e.action == "branding.publish" && e.actor == "operator:ada")
    );

    // The OAuth pages get the checked variables only; the portal gets it all.
    let (_, _, vars) = h
        .raw(
            "GET",
            &format!("/branding/theme/{v1}/vars.css"),
            None,
            vec![],
            &[],
        )
        .await;
    let vars = String::from_utf8(vars).unwrap();
    assert!(
        vars.contains("--primary: #1f6feb") && !vars.contains("brand-blurb"),
        "{vars}"
    );
    let (_, headers, full) = h
        .raw(
            "GET",
            &format!("/branding/theme/{v1}/full.css"),
            None,
            vec![],
            &[],
        )
        .await;
    assert!(String::from_utf8(full).unwrap().contains(".brand-blurb"));
    assert!(
        headers["cache-control"]
            .to_str()
            .unwrap()
            .contains("immutable")
    );
    let p = Browser::default();
    let (_, _, page) = p.get(&h, "/account/sign-in").await;
    assert!(
        page.contains(&format!("/branding/theme/{v1}/full.css"))
            && page.contains(&format!("/branding/{logo_id}")),
        "{page}"
    );
    assert!(
        page.contains("Welcome back")
            && page.contains("Example Social")
            && page.contains("Run by Example &amp; friends"),
        "{page}"
    );
    assert!(page.contains(r#"href="https://example.com/terms""#));
    let html = oauth_sign_in_page(&h).await;
    assert!(
        html.contains(&format!("/branding/theme/{v1}/vars.css")) && !html.contains("full.css"),
        "{html}"
    );
    assert!(
        html.contains(&format!("/branding/{logo_id}")) && html.contains("Example Social"),
        "{html}"
    );
    assert!(
        !html.contains("Welcome back"),
        "no page text on OAuth sign-in"
    );

    // Previews (the editor's), and only for operators.
    let (st, headers, page) = b.get(&h, "/admin/branding/preview/consent?mode=dark").await;
    assert!(
        st == StatusCode::OK
            && page.contains("Example App")
            && page.contains("draft.css?kind=vars&amp;mode=dark"),
        "{page}"
    );
    assert!(
        headers["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'self'")
    );
    let (_, _, page) = b.get(&h, "/admin/branding/preview/portal?mode=dark").await;
    assert!(
        page.contains(r#"class="mode-dark"#) && page.contains("draft.css?kind=full"),
        "{page}"
    );
    assert_eq!(
        p.get(&h, "/admin/branding/preview/consent").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        p.get(&h, "/admin/branding/draft.css?kind=full").await.0,
        StatusCode::NOT_FOUND
    );

    // Emails carry the footer.
    let acct = h
        .create_account("ivy.pds.example.com", "ivy@example.com", "ivys password")
        .await;
    let access = acct["accessJwt"].as_str().unwrap().to_owned();
    h.call(
        "POST",
        "/xrpc/com.atproto.server.requestEmailConfirmation",
        Some(&access),
        None,
    )
    .await;
    let sent = h.mail.lock().unwrap().last().cloned().unwrap();
    assert!(
        sent.email.text.contains("Run by Example & friends")
            && sent.email.html.contains("Run by Example &amp; friends"),
        "{}",
        sent.email.text
    );

    // Another version, then back to the first; then off.
    let second = brand_form(
        &csrf,
        &[("light_primary", "#0b5cad"), ("dark_primary", "#4d8dff")],
    );
    b.post(&h, "/admin/branding/save", &second).await;
    b.post(&h, "/admin/branding/publish", &[("csrf", &csrf)])
        .await;
    let v2 = h
        .state
        .store
        .branding_active()
        .await
        .unwrap()
        .unwrap()
        .version;
    assert!(v2 > v1);
    let v1s = v1.to_string();
    let (st, _, _) = b
        .post(
            &h,
            "/admin/branding/rollback",
            &[("csrf", &csrf), ("version", &v1s)],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(
        h.state
            .store
            .branding_active()
            .await
            .unwrap()
            .unwrap()
            .version,
        v1
    );
    b.post(&h, "/admin/branding/off", &[("csrf", &csrf)]).await;
    let (_, _, page) = p.get(&h, "/account/sign-in").await;
    assert!(
        !page.contains("/branding/theme/"),
        "the built-in look again: {page}"
    );

    // A viewer sees it but can't change it.
    let v = Browser::default();
    h.console_sign_in(&v, "vic", "a viewer password!").await;
    let (st, _, page) = v.get(&h, "/admin/branding").await;
    assert!(
        st == StatusCode::OK && !page.contains("Save the draft"),
        "{page}"
    );
    let vcsrf = hidden(&page, "csrf");
    let (st, _, _) = v
        .post(&h, "/admin/branding/publish", &[("csrf", &vcsrf)])
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

/// An OAuth sign-in page, fetched the way a browser would.
async fn oauth_sign_in_page(h: &Harness) -> String {
    const LB: &str = "http://localhost?scope=atproto";
    let client = DpopClient::new();
    let (_, challenge) = pkce();
    let (_, _, v) = client
        .post(
            h,
            "/oauth/par",
            &[
                ("client_id", LB),
                ("response_type", "code"),
                ("scope", "atproto"),
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
    Browser::default().get(h, &path).await.2
}

#[tokio::test]
async fn the_admin_socket_is_the_owner_and_tcp_can_not_pretend_to_be_it() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let h = harness(false).await;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let secret = h.state.socket_secret.clone();
    // Through the router, the marker is an owner credential; a wrong one is refused.
    let (st, _, _) = h
        .raw(
            "GET",
            &ns("owner.listOperators"),
            None,
            vec![],
            &[(crate::socket::MARKER, &secret)],
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, _) = h
        .raw(
            "GET",
            &ns("owner.listOperators"),
            None,
            vec![],
            &[(crate::socket::MARKER, "guess")],
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Over TCP the marker is stripped, so even the right value is nothing.
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = tcp.local_addr().unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    tokio::spawn(crate::serve(h.state.clone(), tcp, cancel.clone()));
    let res = reqwest::Client::new()
        .get(format!("http://{addr}{}", ns("owner.listOperators")))
        .header(crate::socket::MARKER, &secret)
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status().as_u16(),
        401,
        "the TCP listener strips the marker"
    );

    // Over the socket, a request works as the owner.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("admin.sock");
    let listener = crate::socket::bind(&path).unwrap();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    tokio::spawn(crate::socket::serve(
        h.state.clone(),
        listener,
        path.clone(),
        cancel.clone(),
    ));
    let mut s = tokio::net::UnixStream::connect(&path).await.unwrap();
    let body = r#"{"username":"cli-made","password":"a long enough password","role":"viewer"}"#;
    let req = format!(
        "POST {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        ns("owner.createOperator"),
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    assert!(out.starts_with("HTTP/1.1 200"), "{out}");
    assert!(h.state.store.operator("cli-made").await.unwrap().is_some());
    let audit = h.state.store.audit_entries(None, 10).await.unwrap();
    assert!(
        audit
            .iter()
            .any(|e| e.action == "operator.create" && e.actor == "socket")
    );
    cancel.cancel();
}

#[tokio::test]
async fn setup_applies_on_a_database_no_node_has_started() {
    let h = setup_harness("pds.example.com", false).await;
    assert!(
        h.state.store.setup_state().await.unwrap().is_none(),
        "no setup row yet"
    );
    let mut plan = crate::setup::seed_plan(&h.state.snapshot(), Some("pds.example.com"));
    plan.admin_password = Some("an admin password".into());
    plan.email.skipped = true;
    plan.keys_backup_acknowledged = true;
    crate::setup::apply(&h.state.store, &h.state.master_key, &plan, false)
        .await
        .unwrap();
    assert!(h.state.store.setup_completed().await.unwrap());
}

#[tokio::test]
async fn health_and_the_certificate_question() {
    let h = harness(false).await;
    let (st, v) = h.call("GET", "/xrpc/_health", None, None).await;
    assert_eq!((st, v["version"].is_string()), (StatusCode::OK, true));
    assert_eq!(
        h.call("GET", "/healthz", None, None).await.0,
        StatusCode::OK
    );
    assert_eq!(h.call("GET", "/readyz", None, None).await.0, StatusCode::OK);
    // Health answers on any host, as the probes use internal names.
    let (st, _) = h
        .call_host("GET", "/xrpc/_health", None, None, "unknown.example.net")
        .await;
    assert_eq!(st, StatusCode::OK);

    h.create_account("hana.pds.example.com", "hana@example.com", "hanas password")
        .await;
    let check = |name: &'static str| {
        let h = &h;
        async move {
            h.call_host(
                "GET",
                &format!("/tls-check?domain={name}"),
                None,
                None,
                "localhost",
            )
            .await
            .0
        }
    };
    assert_eq!(
        check("pds.example.com").await,
        StatusCode::OK,
        "the PDS hostname"
    );
    assert_eq!(
        check("PDS.Example.com.").await,
        StatusCode::OK,
        "in any case, with the root dot"
    );
    assert_eq!(
        check("hana.pds.example.com").await,
        StatusCode::OK,
        "an existing handle"
    );
    assert_eq!(
        check("nobody.pds.example.com").await,
        StatusCode::NOT_FOUND,
        "a handle no one has"
    );
    assert_eq!(check("www.example.com").await, StatusCode::NOT_FOUND);
    assert_eq!(check("").await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn behind_cloudflare_the_client_ip_comes_from_cloudflare_only() {
    let h = harness(false).await;
    let ip = |s: &str| s.parse::<std::net::IpAddr>().unwrap();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-forwarded-for", "104.16.1.2".parse().unwrap());
    headers.insert("cf-connecting-ip", "203.0.113.9".parse().unwrap());
    let caddy = Some(ip("127.0.0.1"));
    // Not in Cloudflare mode: CF-Connecting-IP means nothing.
    assert_eq!(
        crate::ratelimit::real_ip(&h.state, caddy, &headers),
        Some(ip("104.16.1.2"))
    );
    h.state
        .store
        .set_setting("deploy.proxy_mode", json!("cloudflare"), "test")
        .await
        .unwrap();
    h.state.settings.refresh().await.unwrap();
    assert_eq!(
        crate::ratelimit::real_ip(&h.state, caddy, &headers),
        Some(ip("203.0.113.9"))
    );
    // The hop before the proxy isn't Cloudflare's: the header is ignored.
    headers.insert("x-forwarded-for", "198.51.100.7".parse().unwrap());
    assert_eq!(
        crate::ratelimit::real_ip(&h.state, caddy, &headers),
        Some(ip("198.51.100.7"))
    );
    // A client talking to locus directly can't claim to be anyone.
    headers.insert("x-forwarded-for", "104.16.1.2".parse().unwrap());
    assert_eq!(
        crate::ratelimit::real_ip(&h.state, Some(ip("192.0.2.50")), &headers),
        Some(ip("192.0.2.50"))
    );
}

// ---------------------------------------------------------------- keys

impl Harness {
    /// An owner with a real password, for calls that re-authenticate.
    async fn owner_with_password(&self, name: &str, password: &str) -> String {
        let hash = locus_auth::password::hash(password).unwrap();
        assert!(
            self.state
                .store
                .create_operator(name, &hash, "owner")
                .await
                .unwrap()
        );
        let token = crate::admin_auth::new_token();
        self.state
            .store
            .create_operator_token(
                &format!("t-{name}"),
                &crate::admin_auth::token_hash(&token),
                name,
                "test",
                None,
            )
            .await
            .unwrap();
        format!("Bearer {token}")
    }

    async fn backup_bytes(&self, auth: &str, passphrase: &str) -> (StatusCode, Vec<u8>) {
        let path = format!("/xrpc/{}.owner.keys.backup", locus_core::LOCUS_NS);
        let (st, _, out) = self
            .raw(
                "POST",
                &path,
                None,
                serde_json::to_vec(&json!({"passphrase": passphrase})).unwrap(),
                &[
                    ("authorization", auth),
                    ("content-type", "application/json"),
                ],
            )
            .await;
        (st, out)
    }
}

#[tokio::test]
async fn key_backups_hold_every_key_and_make_a_pending_plc_key_current() {
    use locus_core::key_backup::Entry;
    let h = harness(false).await;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let (did, _) = h.user("kim.pds.example.com").await;
    let owner = h.owner_with_password("olga", "olga-password-123").await;
    let admin = format!("Bearer {}", h.operator("adam", "admin").await);
    let first = h.state.secrets.plc_keys.current_did().unwrap();

    // Owners only.
    assert_eq!(
        h.admin_call("GET", &ns("owner.keys.show"), &admin, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (st, v) = h
        .admin_call("GET", &ns("owner.keys.show"), &owner, None)
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["plcRotationKeys"][0]["didKey"], json!(first));
    assert_eq!(v["plcRotationKeys"][0]["inLastBackup"], json!(false));
    assert!(v["lastBackup"].is_null());

    // A backup: every secret, every account key and the PLC keys' roles.
    let (st, bytes) = h.backup_bytes(&owner, "short").await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "passphrases are at least 12 characters"
    );
    assert!(bytes.starts_with(b"{\"error\""));
    let (st, bytes) = h.backup_bytes(&owner, "backup passphrase 1").await;
    assert_eq!(st, StatusCode::OK);
    let (header, entries) = locus_core::key_backup::open(&bytes, "backup passphrase 1").unwrap();
    assert_eq!(header.service_did, h.state.service_did);
    assert_eq!(header.master_key_id, h.state.master_key.key_id());
    let has = |f: &dyn Fn(&Entry) -> bool| entries.iter().any(f);
    assert!(has(
        &|e| matches!(e, Entry::PlcKey { did_key, role, .. } if *did_key == first && role == "current")
    ));
    assert!(has(
        &|e| matches!(e, Entry::Secret { name, .. } if *name == format!("plc_rotation_key/{first}"))
    ));
    assert!(has(
        &|e| matches!(e, Entry::Secret { name, .. } if name == "service_signing_key")
    ));
    let stored = h
        .state
        .store
        .get_signing_key(&did, &h.state.master_key)
        .await
        .unwrap()
        .unwrap();
    assert!(has(
        &|e| matches!(e, Entry::SigningKey { did: d, value, .. } if *d == did && **value == *stored.1)
    ));
    assert!(locus_core::key_backup::open(&bytes, "backup passphrase 2").is_err());
    let state = h.state.store.key_state().await.unwrap();
    assert_eq!(state.last_backup_by.as_deref(), Some("operator:olga"));

    // A new PLC rotation key needs re-authentication, and is pending.
    let set = ns("owner.keys.setPlcRotationKey");
    let (st, v) = h
        .admin_call("POST", &set, &owner, Some(json!({"generate": true})))
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("ReauthRequired"))
    );
    let (st, v) = h
        .admin_call(
            "POST",
            &set,
            &owner,
            Some(json!({"generate": true, "password": "wrong password 1"})),
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "{v}");
    let reauth = json!({"password": "olga-password-123"});
    let (st, v) = h
        .admin_call(
            "POST",
            &set,
            &owner,
            Some(json!({"generate": true, "password": "olga-password-123"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let pending = v["didKey"].as_str().unwrap().to_owned();
    assert_eq!(v["role"], "pending");
    let generation = h.state.store.key_state().await.unwrap().generation;
    assert!(generation > state.generation, "nodes reload");
    // Neither or two actions at once are refused.
    let (st, _) = h
        .admin_call(
            "POST",
            &set,
            &owner,
            Some(merge(
                json!({"generate": true, "discardPending": true}),
                &reauth,
            )),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // A backup made after it makes it current; the old key stays secondary.
    let (st, bytes) = h.backup_bytes(&owner, "backup passphrase 1").await;
    assert_eq!(st, StatusCode::OK);
    let (_, entries) = locus_core::key_backup::open(&bytes, "backup passphrase 1").unwrap();
    assert!(entries.iter().any(|e| matches!(e, Entry::Secret { name, .. } if *name == format!("plc_rotation_key/{pending}"))));
    let rows = h.state.store.plc_key_rows().await.unwrap();
    let role = |k: &str| rows.iter().find(|r| r.did_key == k).map(|r| r.role.clone());
    assert_eq!(role(&pending).as_deref(), Some("current"));
    assert_eq!(role(&first).as_deref(), Some("secondary"));

    // The old key can't be dropped while an account's document lists only it.
    let drop = ns("owner.keys.dropPlcKey");
    let (st, v) = h
        .admin_call(
            "POST",
            &drop,
            &owner,
            Some(merge(json!({"didKey": first}), &reauth)),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["message"].as_str().unwrap().contains(&did), "{v}");
    assert_eq!(
        h.state.store.plc_key_listing(&did).await.unwrap(),
        Some(vec![first.clone()]),
        "the guard recorded what it saw"
    );
    let (st, _) = h
        .admin_call(
            "POST",
            &drop,
            &owner,
            Some(merge(json!({"didKey": pending}), &reauth)),
        )
        .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "the current key can't be dropped"
    );

    // A pending key nobody lists can be discarded; another can be made
    // current without a backup, explicitly.
    for _ in 0..2 {
        let (st, _) = h
            .admin_call(
                "POST",
                &set,
                &owner,
                Some(merge(json!({"generate": true}), &reauth)),
            )
            .await;
        assert_eq!(st, StatusCode::OK);
    }
    let (st, v) = h
        .admin_call(
            "POST",
            &set,
            &owner,
            Some(merge(json!({"discardPending": true}), &reauth)),
        )
        .await;
    assert_eq!(
        (st, v["role"].as_str()),
        (StatusCode::OK, Some("discarded")),
        "{v}"
    );
    let (st, _) = h
        .admin_call(
            "POST",
            &set,
            &owner,
            Some(merge(json!({"discardPending": true}), &reauth)),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "nothing pending");
    let hex_key = {
        use proto_blue_crypto::ExportableKeypair as _;
        hex::encode(proto_blue_crypto::K256Keypair::generate().export_private_key())
    };
    let (st, v) = h
        .admin_call(
            "POST",
            &set,
            &owner,
            Some(merge(json!({"privateKeyHex": hex_key}), &reauth)),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let third = v["didKey"].as_str().unwrap().to_owned();
    let (st, v) = h
        .admin_call(
            "POST",
            &set,
            &owner,
            Some(merge(json!({"acknowledgeNoBackup": true}), &reauth)),
        )
        .await;
    assert_eq!(
        (st, v["role"].as_str()),
        (StatusCode::OK, Some("current")),
        "{v}"
    );
    let (_, v) = h
        .admin_call("GET", &ns("owner.keys.show"), &owner, None)
        .await;
    let shown: Vec<(String, String)> = v["plcRotationKeys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["didKey"].as_str().unwrap().to_owned(),
                r["role"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert!(shown.contains(&(third.clone(), "current".into())), "{v}");
    assert_eq!(shown.len(), 3, "first and second stay secondary: {v}");
    assert!(
        h.state
            .store
            .audit_entries(None, 50)
            .await
            .unwrap()
            .iter()
            .any(|a| a.action == "keys.plcRotationKey.current"),
        "audited"
    );
}

fn merge(mut a: Value, b: &Value) -> Value {
    if let (Some(a), Some(b)) = (a.as_object_mut(), b.as_object()) {
        a.extend(b.clone());
    }
    a
}

#[tokio::test]
async fn a_changed_env_plc_key_becomes_pending_once() {
    let h = harness(false).await;
    let first = h.state.secrets.plc_keys.current_did().unwrap();
    let env_key = proto_blue_crypto::K256Keypair::generate();
    let rows = |h: &Harness| {
        let s = h.state.store.clone();
        async move {
            s.plc_key_rows()
                .await
                .unwrap()
                .into_iter()
                .map(|r| (r.did_key, r.role))
                .collect::<Vec<_>>()
        }
    };
    let env_private = env_key.export_private_key();
    crate::state::apply_env_plc_key(
        &h.state.store,
        &h.state.master_key,
        &env_key.did(),
        locus_store::keys::PlcKeySource::Private(&env_private),
    )
    .await
    .unwrap();
    let after = rows(&h).await;
    assert!(
        after.contains(&(env_key.did(), "pending".into())),
        "{after:?}"
    );
    assert!(after.contains(&(first.clone(), "current".into())));
    // Applied once: a later boot with the same value changes nothing, even
    // after the key moved on.
    let mut tx = h.state.store.begin().await.unwrap();
    tx.promote_pending_plc_key().await.unwrap();
    tx.commit().await.unwrap();
    let mut tx = h.state.store.begin().await.unwrap();
    let other = proto_blue_crypto::K256Keypair::generate();
    use proto_blue_crypto::ExportableKeypair as _;
    tx.store_pending_plc_key(
        &other.did(),
        locus_store::keys::PlcKeySource::Private(&other.export_private_key()),
        &h.state.master_key,
    )
    .await
    .unwrap();
    tx.promote_pending_plc_key().await.unwrap();
    tx.commit().await.unwrap();
    let before = rows(&h).await;
    let env_private = env_key.export_private_key();
    crate::state::apply_env_plc_key(
        &h.state.store,
        &h.state.master_key,
        &env_key.did(),
        locus_store::keys::PlcKeySource::Private(&env_private),
    )
    .await
    .unwrap();
    assert_eq!(
        rows(&h).await,
        before,
        "an unchanged env value never overrides a later change"
    );
}

impl Harness {
    /// Stores a new signing key for `did`, returning it.
    async fn swap_signing_key(&self, did: &str) -> proto_blue_crypto::K256Keypair {
        use proto_blue_crypto::ExportableKeypair as _;
        let k = proto_blue_crypto::K256Keypair::generate();
        let mut tx = self.state.store.begin().await.unwrap();
        tx.put_signing_key(did, "k256", &k.export_private_key(), &self.state.master_key)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        k
    }

    fn plc_signing_key(&self, did: &str) -> Option<String> {
        self.plc.lock().unwrap().last(did).and_then(|op| {
            op["verificationMethods"]["atproto"]
                .as_str()
                .map(str::to_owned)
        })
    }
}

#[tokio::test]
async fn account_keys_rotate_identities_publish_and_secrets_change() {
    use proto_blue_crypto::Keypair as _;
    let h = harness(false).await;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let (did, _) = h.user("lou.pds.example.com").await;
    let owner = h.owner_with_password("ola", "ola-password-1234").await;
    let reauth = json!({"password": "ola-password-1234"});

    // rotate-keys: the PLC document follows the stored key, then #identity
    // and a fresh commit's #sync.
    let stored = h.swap_signing_key(&did).await;
    assert_ne!(h.plc_signing_key(&did), Some(stored.did()));
    let root_before = h.state.store.repo_root_of(&did).await.unwrap().unwrap();
    let (st, v) = h
        .admin_call(
            "POST",
            &ns("admin.keys.rotateAccountKeys"),
            &owner,
            Some(json!({"dids": [did, "did:plc:nobodyhere"]})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["results"][0]["plcUpdated"], json!(true), "{v}");
    assert_eq!(v["results"][1]["ok"], json!(false));
    assert_eq!(h.plc_signing_key(&did), Some(stored.did()));
    let kinds = h.outbox_kinds(&did).await;
    assert_eq!(&kinds[kinds.len() - 2..], ["identity", "sync"]);
    let root = h.state.store.repo_root_of(&did).await.unwrap().unwrap();
    assert_eq!(root.data_cid, root_before.data_cid, "same tree");
    assert_ne!(root.commit_cid, root_before.commit_cid, "a new commit");
    let commit = h
        .state
        .store
        .block(&did, &root.commit_cid)
        .await
        .unwrap()
        .unwrap();
    let parsed = proto_blue_repo::commit::SignedCommit::from_lex_value(
        &proto_blue_lex_cbor::decode(&commit).unwrap(),
    )
    .unwrap();
    assert!(
        proto_blue_repo::commit::verify_commit_sig(&parsed, &stored.did()).unwrap(),
        "signed with the stored key"
    );
    // Already up to date: no PLC op, still announced.
    let ops = h.plc.lock().unwrap().log[&did].len();
    let (_, v) = h
        .admin_call(
            "POST",
            &ns("admin.keys.rotateAccountKeys"),
            &owner,
            Some(json!({"dids": [did]})),
        )
        .await;
    assert_eq!(v["results"][0]["plcUpdated"], json!(false), "{v}");
    assert_eq!(h.plc.lock().unwrap().log[&did].len(), ops);

    // publish-identity.
    let before = h.outbox_kinds(&did).await.len();
    let (st, _) = h
        .admin_call(
            "POST",
            &ns("admin.identity.publish"),
            &owner,
            Some(json!({"dids": [did]})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let kinds = h.outbox_kinds(&did).await;
    assert_eq!(
        (kinds.len(), kinds.last().map(String::as_str)),
        (before + 1, Some("identity"))
    );

    // keys set jwt-secret / dpop-secret.
    let set = ns("owner.keys.setSecret");
    let call = |body: Value| h.admin_call("POST", &set, &owner, Some(merge(body, &reauth)));
    assert_eq!(
        call(json!({"name": "jwt-secret", "value": "short"}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(json!({"name": "dpop-secret", "value": "nothex"}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(json!({"name": "hostname", "value": "x"})).await.0,
        StatusCode::BAD_REQUEST
    );
    let jwt = "a new jwt secret that is long enough!!";
    let (st, v) = call(json!({"name": "jwt-secret", "value": jwt})).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        h.state
            .store
            .get_secret("jwt_secret", &h.state.master_key)
            .await
            .unwrap()
            .unwrap()
            .as_slice(),
        jwt.as_bytes()
    );
    let audit = h.state.store.audit_entries(None, 20).await.unwrap();
    let row = audit.iter().find(|a| a.action == "keys.setSecret").unwrap();
    assert!(
        !row.detail_json.clone().unwrap_or_default().contains(jwt),
        "only a fingerprint is audited"
    );
    let (st, _) = call(json!({"name": "dpop-secret", "value": "11".repeat(32)})).await;
    assert_eq!(st, StatusCode::OK);

    // rotate-service-key.
    let old = h.state.secrets.service_key.did();
    let (st, v) = h
        .admin_call(
            "POST",
            &ns("owner.keys.rotateServiceKey"),
            &owner,
            Some(reauth.clone()),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let stored_service = h
        .state
        .store
        .get_secret("service_signing_key", &h.state.master_key)
        .await
        .unwrap()
        .unwrap();
    let new = proto_blue_crypto::K256Keypair::from_private_key(&stored_service).unwrap();
    assert_eq!(v["didKey"], json!(new.did()));
    assert_ne!(new.did(), old);
}

#[tokio::test]
async fn key_imports_restore_what_live_documents_list() {
    use proto_blue_crypto::{ExportableKeypair as _, Keypair as _};
    let h = harness(false).await;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let (did, _) = h.user("max.pds.example.com").await;
    let (other, _) = h.user("ned.pds.example.com").await;
    let owner = h.owner_with_password("oz", "oz-password-12345").await;
    let rotate_path = ns("admin.keys.rotateAccountKeys");
    let rotate = |dids: Vec<&String>| {
        h.admin_call("POST", &rotate_path, &owner, Some(json!({"dids": dids})))
    };
    let k1 = h.state.signing_key(&did).await.unwrap();
    // A held secondary PLC key, in the backup, later lost.
    let extra = proto_blue_crypto::K256Keypair::generate();
    let mut tx = h.state.store.begin().await.unwrap();
    assert!(
        tx.insert_secondary_plc_key(
            &extra.did(),
            locus_store::keys::PlcKeySource::Private(&extra.export_private_key()),
            &h.state.master_key
        )
        .await
        .unwrap()
    );
    tx.commit().await.unwrap();
    // The account's key changes after the snapshot: K2 is stored and listed.
    let k2 = h.swap_signing_key(&did).await;
    assert_eq!(rotate(vec![&did]).await.0, StatusCode::OK);
    let (st, backup) = h.backup_bytes(&owner, "import passphrase").await;
    assert_eq!(st, StatusCode::OK);
    // The other account: the backup has its key, but its document moves on
    // to a key the backup doesn't have, and the stored one is a third.
    let _k4 = h.swap_signing_key(&other).await;
    rotate(vec![&other]).await;
    let _k5 = h.swap_signing_key(&other).await;
    // A restore brings the old state back: K1 stored, the extra key gone.
    let mut tx = h.state.store.begin().await.unwrap();
    tx.put_signing_key(&did, "k256", &k1.export_private_key(), &h.state.master_key)
        .await
        .unwrap();
    tx.remove_plc_key(&extra.did()).await.unwrap();
    tx.commit().await.unwrap();

    let body = |head: Value| {
        let mut b = head.to_string().into_bytes();
        b.push(b'\n');
        b.extend_from_slice(&backup);
        b
    };
    let post = |bytes: Vec<u8>| {
        let h = &h;
        let owner = owner.clone();
        async move {
            let (st, _, out) = h
                .raw(
                    "POST",
                    &format!("/xrpc/{}.owner.keys.import", locus_core::LOCUS_NS),
                    None,
                    bytes,
                    &[
                        ("authorization", &owner),
                        ("content-type", "application/octet-stream"),
                    ],
                )
                .await;
            (
                st,
                serde_json::from_slice::<Value>(&out).unwrap_or(Value::Null),
            )
        }
    };
    let (st, _) = post(body(json!({"passphrase": "import passphrase"}))).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "re-authentication first");
    let (st, _) = post(body(
        json!({"passphrase": "the wrong one!!", "password": "oz-password-12345"}),
    ))
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let syncs_before = h
        .outbox_kinds(&did)
        .await
        .iter()
        .filter(|k| *k == "sync")
        .count();
    let (st, v) = post(body(
        json!({"passphrase": "import passphrase", "password": "oz-password-12345"}),
    ))
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["plcKeysAdded"], json!([extra.did()]), "{v}");
    assert_eq!(v["signingKeysReplaced"], json!([did]), "{v}");
    assert_eq!(v["listNeitherKey"][0]["did"], json!(other), "{v}");
    assert_eq!(h.state.signing_key(&did).await.unwrap().did(), k2.did());
    assert_eq!(
        h.outbox_kinds(&did)
            .await
            .iter()
            .filter(|k| *k == "sync")
            .count(),
        syncs_before + 1,
        "a fresh commit's #sync for the active account"
    );
    let held = h.state.store.plc_key_rows().await.unwrap();
    assert!(
        held.iter()
            .any(|r| r.did_key == extra.did() && r.role == "secondary")
    );
    // Run again: nothing more to do.
    let (_, v) = post(body(
        json!({"passphrase": "import passphrase", "password": "oz-password-12345"}),
    ))
    .await;
    assert_eq!(
        (v["plcKeysAdded"].clone(), v["signingKeysReplaced"].clone()),
        (json!([]), json!([]))
    );
}

/// A new secp256k1 key in the test KMS (`LOCUS_KMS_ENDPOINT`, local-kms),
/// or None to skip. local-kms takes unsigned calls.
async fn test_kms_k256_key() -> Option<String> {
    let endpoint = std::env::var("LOCUS_KMS_ENDPOINT")
        .ok()
        .filter(|v| !v.is_empty())?;
    let v: Value = reqwest::Client::new()
        .post(endpoint)
        .header("X-Amz-Target", "TrentService.CreateKey")
        .header("Content-Type", "application/x-amz-json-1.1")
        .body(r#"{"KeySpec":"ECC_SECG_P256K1","KeyUsage":"SIGN_VERIFY"}"#)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    Some(v["KeyMetadata"]["KeyId"].as_str().unwrap().to_owned())
}

#[tokio::test]
async fn a_kms_plc_rotation_key_signs_in_kms() {
    use locus_core::key_backup::Entry;
    let Some(key_id) = test_kms_k256_key().await else {
        eprintln!("LOCUS_KMS_ENDPOINT isn't set; skipping");
        return;
    };
    let h = harness(false).await;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let owner = h.owner_with_password("kim", "kim-password-1234").await;
    let set = ns("owner.keys.setPlcRotationKey");
    let reauth = json!({"password": "kim-password-1234"});
    let (st, v) = h
        .admin_call(
            "POST",
            &set,
            &owner,
            Some(merge(json!({"kmsKeyId": key_id}), &reauth)),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let did_key = v["didKey"].as_str().unwrap().to_owned();
    assert!(did_key.starts_with("did:key:zQ3s"));
    let row = h
        .state
        .store
        .plc_key_rows()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.did_key == did_key)
        .unwrap();
    assert_eq!(row.kms_key_id.as_deref(), Some(key_id.as_str()));
    assert!(
        h.state
            .store
            .get_secret(&format!("plc_rotation_key/{did_key}"), &h.state.master_key)
            .await
            .unwrap()
            .is_none(),
        "no private half here"
    );
    // A backup names it by its KMS id.
    let (_, bytes) = h.backup_bytes(&owner, "kms backup passphrase").await;
    let (_, entries) = locus_core::key_backup::open(&bytes, "kms backup passphrase").unwrap();
    assert!(entries.iter().any(|e| matches!(
        e,
        Entry::PlcKey { did_key: d, kms_key_id: Some(k), .. } if *d == did_key && *k == key_id
    )));
    // Current (the backup made it so); a node loading the keys signs in KMS.
    let keys = crate::state::PlcKeys::load_or_create(&h.state.store, &h.state.master_key)
        .await
        .unwrap();
    let current = keys.current().unwrap();
    assert_eq!(current.did(), did_key);
    assert!(matches!(current, crate::state::PlcSigner::Kms { .. }));
    let op = locus_identity::plc::Operation {
        rotation_keys: vec![did_key.clone()],
        verification_methods: [("atproto".to_owned(), did_key.clone())].into(),
        also_known_as: vec!["at://kms.pds.example.com".into()],
        services: std::collections::BTreeMap::new(),
        prev: None,
    };
    let signed = current.sign_op(&op).await.unwrap();
    let sig = match &signed {
        locus_identity::plc::SignedOp::Operation { sig, .. } => sig.clone(),
        other => panic!("{other:?}"),
    };
    let raw =
        base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, sig).unwrap();
    assert!(
        proto_blue_crypto::verify_signature(&did_key, &op.signing_bytes().unwrap(), &raw, false)
            .unwrap()
    );
}

#[tokio::test]
async fn backups_hold_the_database_every_object_it_names_and_the_keys() {
    let h = harness(false).await;
    if sqlite_pool(&h.state.store).is_none() {
        return; // `backup create` is for SQLite installs (Postgres has PITR)
    }
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let (did, access) = h.user("bob.pds.example.com").await;
    let blob = h.upload(&access, b"backed up bytes", "image/png").await;
    let cid = blob["ref"]["$link"].as_str().unwrap().to_owned();
    let row = h.state.store.blob(&did, &cid).await.unwrap().unwrap();
    let owner = format!("Bearer {}", h.operator("bea", "owner").await);
    let admin = format!("Bearer {}", h.operator("ada", "admin").await);
    assert_eq!(
        h.admin_call("POST", &ns("owner.backup.create"), &admin, Some(json!({})))
            .await
            .0,
        StatusCode::FORBIDDEN,
        "owners only"
    );
    let (st, v) = h
        .admin_call(
            "POST",
            &ns("owner.backup.create"),
            &owner,
            Some(json!({"passphrase": "backup passphrase 9"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    let dir = std::path::PathBuf::from(v["dir"].as_str().unwrap());
    // One at a time.
    let (st, _) = h
        .admin_call("POST", &ns("owner.backup.create"), &owner, Some(json!({})))
        .await;
    assert!(st == StatusCode::BAD_REQUEST || st == StatusCode::OK);
    let mut status = String::new();
    for _ in 0..100 {
        let (_, l) = h
            .admin_call("GET", &ns("viewer.backup.list"), &owner, None)
            .await;
        let run = l["backups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == json!(id))
            .cloned()
            .unwrap();
        status = run["status"].as_str().unwrap().to_owned();
        if status != "running" {
            assert_eq!(run["keysBackup"], json!(true), "{run}");
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(status, "done");
    let manifest: crate::backups::Manifest =
        serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(
        (
            manifest.status.as_str(),
            manifest.objects,
            manifest.missing.len()
        ),
        ("done", 1, 0)
    );
    assert_eq!(manifest.master_key_id, h.state.master_key.key_id());
    assert_eq!(
        std::fs::read(dir.join("objects").join(&row.storage_key)).unwrap(),
        b"backed up bytes"
    );
    let snap = locus_store::Store::connect(&locus_core::bootstrap::DatabaseUrl::Sqlite(
        dir.join("db.sqlite"),
    ))
    .await
    .unwrap();
    assert!(
        snap.account_by_did(&did).await.unwrap().is_some(),
        "the database is in it"
    );
    snap.close().await;
    let keys = std::fs::read(dir.join("keys.backup")).unwrap();
    assert!(locus_core::key_backup::open(&keys, "backup passphrase 9").is_ok());
    // A backup without a passphrase has no key backup.
    let (st, v) = h
        .admin_call("POST", &ns("owner.backup.create"), &owner, Some(json!({})))
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let dir2 = std::path::PathBuf::from(v["dir"].as_str().unwrap());
    for _ in 0..100 {
        if dir2.join("manifest.json").exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(!dir2.join("keys.backup").exists());
}

#[tokio::test]
async fn blob_migrations_copy_verify_pause_and_flip() {
    use crate::blob_migration::{Step, step};
    use locus_store::migrate_blobs::MIGRATE_LEASE;
    let h = harness(false).await;
    if !matches!(h.state.blobs.name(), "disk") {
        return;
    }
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let (did, access) = h.user("mia.pds.example.com").await;
    let mut cids = Vec::new();
    for n in 0..3u8 {
        let b = h
            .upload(&access, format!("moving bytes {n}").as_bytes(), "image/png")
            .await;
        cids.push(b["ref"]["$link"].as_str().unwrap().to_owned());
    }
    let admin = format!("Bearer {}", h.operator("amy", "admin").await);
    let target = tempfile::tempdir().unwrap();
    let root = target.path().display().to_string();
    let (st, v) = h
        .admin_call(
            "POST",
            &ns("admin.migrateBlobs.start"),
            &admin,
            Some(json!({"to": "disk", "diskRoot": root})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (st, _) = h
        .admin_call(
            "POST",
            &ns("admin.migrateBlobs.start"),
            &admin,
            Some(json!({"to": "disk", "diskRoot": root})),
        )
        .await;
    assert!(st.is_client_error(), "one at a time: {st}");
    let lease = h
        .state
        .store
        .try_acquire_lease(
            MIGRATE_LEASE,
            "test-node",
            std::time::Duration::from_secs(60),
        )
        .await
        .unwrap()
        .unwrap();

    // While paused, an upload is refused (retryable) and leaves no row.
    let m = h
        .state
        .store
        .active_blob_migration()
        .await
        .unwrap()
        .unwrap();
    assert!(
        h.state
            .store
            .pause_for_cutover(&lease, &m.id)
            .await
            .unwrap()
    );
    let (st, _, _) = h
        .raw(
            "POST",
            "/xrpc/com.atproto.repo.uploadBlob",
            Some(&access),
            b"during the pause".to_vec(),
            &[("content-type", "image/png")],
        )
        .await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
    let paused_cid = proto_blue_lex_data::Cid::for_raw(b"during the pause").to_string_base32();
    assert!(
        h.state
            .store
            .blob(&did, &paused_cid)
            .await
            .unwrap()
            .is_none()
    );
    h.state.store.clear_pause(&lease).await.unwrap();

    // One step copies everything, verifies, pauses and flips.
    let flipped = step(&h.state, &lease).await.unwrap();
    assert!(matches!(flipped, Step::Flipped(_)), "{flipped:?}");
    for cid in &cids {
        let row = h.state.store.blob(&did, cid).await.unwrap().unwrap();
        assert!(
            target.path().join(&row.storage_key).exists(),
            "{} copied",
            row.storage_key
        );
    }
    let backend = h.state.store.blob_backend().await.unwrap();
    assert_eq!(
        (
            backend.active.as_str(),
            backend.disk_root.as_deref(),
            backend.paused
        ),
        ("disk", Some(root.as_str()), false)
    );
    assert_eq!(backend.epoch, h.state.blobs.epoch() + 1);
    assert_eq!(step(&h.state, &lease).await.unwrap(), Step::Done);
    let (_, v) = h
        .admin_call("GET", &ns("viewer.migrateBlobs.status"), &admin, None)
        .await;
    assert_eq!(v["migration"]["phase"], "done", "{v}");
    // This node's store is the old backend now: writes are refused and it
    // reloads.
    let (st, _, _) = h
        .raw(
            "POST",
            "/xrpc/com.atproto.repo.uploadBlob",
            Some(&access),
            b"after the flip".to_vec(),
            &[("content-type", "image/png")],
        )
        .await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert!(
        h.state.reload.is_cancelled(),
        "the node reloads onto the new backend"
    );
}

#[tokio::test]
async fn blob_migrations_wait_on_missing_sources_and_cancel() {
    use crate::blob_migration::{Step, step};
    use locus_store::migrate_blobs::MIGRATE_LEASE;
    let h = harness(false).await;
    if !matches!(h.state.blobs.name(), "disk") {
        return;
    }
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let (did, access) = h.user("noa.pds.example.com").await;
    let b = h.upload(&access, b"soon missing", "image/png").await;
    let cid = b["ref"]["$link"].as_str().unwrap().to_owned();
    let row = h.state.store.blob(&did, &cid).await.unwrap().unwrap();
    h.state.blobs.delete(&row.storage_key).await.unwrap();
    let admin = format!("Bearer {}", h.operator("ava", "admin").await);
    let target = tempfile::tempdir().unwrap();
    let start = json!({"to": "disk", "diskRoot": target.path().display().to_string()});
    let (st, _) = h
        .admin_call(
            "POST",
            &ns("admin.migrateBlobs.start"),
            &admin,
            Some(start.clone()),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let lease = h
        .state
        .store
        .try_acquire_lease(
            MIGRATE_LEASE,
            "test-node",
            std::time::Duration::from_secs(60),
        )
        .await
        .unwrap()
        .unwrap();
    let waiting = step(&h.state, &lease).await.unwrap();
    assert!(matches!(waiting, Step::Waiting(_)), "{waiting:?}");
    let (_, v) = h
        .admin_call("GET", &ns("viewer.migrateBlobs.status"), &admin, None)
        .await;
    assert_eq!(v["missingSources"][0]["key"], json!(row.storage_key), "{v}");
    assert_eq!(h.state.store.blob_backend().await.unwrap().active, "disk");
    assert!(
        !h.state.store.blob_backend().await.unwrap().paused,
        "no pause while waiting"
    );
    // Cancel: no flip, the lease goes, nothing paused.
    let (st, _) = h
        .admin_call(
            "POST",
            &ns("admin.migrateBlobs.cancel"),
            &admin,
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    // The cancelled job can't go on: its fenced transactions fail, and a
    // step finds no migration.
    assert!(
        h.state
            .store
            .record_copy(&lease, &row.storage_key, "copied")
            .await
            .is_err()
    );
    assert!(
        h.state
            .store
            .active_blob_migration()
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(step(&h.state, &lease).await.unwrap(), Step::Idle);
    assert!(!h.state.store.blob_backend().await.unwrap().paused);
    // Again, then acknowledge the missing object: the cutover goes ahead.
    let (st, _) = h
        .admin_call("POST", &ns("admin.migrateBlobs.start"), &admin, Some(start))
        .await;
    assert_eq!(st, StatusCode::OK);
    let lease = h
        .state
        .store
        .try_acquire_lease(
            MIGRATE_LEASE,
            "test-node",
            std::time::Duration::from_secs(60),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        step(&h.state, &lease).await.unwrap(),
        Step::Waiting(_)
    ));
    let (st, v) = h
        .admin_call(
            "POST",
            &ns("admin.migrateBlobs.acceptMissing"),
            &admin,
            Some(json!({})),
        )
        .await;
    assert_eq!(
        (st, v["acknowledged"].as_i64()),
        (StatusCode::OK, Some(1)),
        "{v}"
    );
    let flipped = step(&h.state, &lease).await.unwrap();
    assert!(matches!(flipped, Step::Flipped(_)), "{flipped:?}");
}

impl Harness {
    /// A request to `host` (not the PDS hostname).
    async fn raw_at(
        &self,
        host: &str,
        method: &str,
        path: &str,
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let req = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", host)
            .body(Body::empty())
            .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let (status, headers) = (res.status(), res.headers().clone());
        let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
        (status, headers, bytes)
    }

    async fn reload_hosts(&self) {
        let hosts = crate::hostnames::Hosts::load(&self.state.store, &self.state.hostname)
            .await
            .unwrap();
        self.state.hosts.store(Arc::new(hosts));
    }
}

#[tokio::test]
async fn hostname_candidates_are_reserved_and_serve_only_health() {
    let h = harness(false).await;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let (_, access) = h.user("pia.pds.example.com").await;
    let owner = h.owner_with_password("oto", "oto-password-1234").await;
    let prepare_path = ns("owner.hostname.prepare");
    let prepare =
        |host: &str| h.admin_call("POST", &prepare_path, &owner, Some(json!({"host": host})));
    assert_eq!(
        prepare("pia.pds.example.com").await.0,
        StatusCode::BAD_REQUEST,
        "a handle host"
    );
    assert_eq!(
        prepare("pds.example.com").await.0,
        StatusCode::BAD_REQUEST,
        "the hostname"
    );
    let (st, v) = prepare("next.pds.example.com").await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(
        prepare("other.example.com").await.0.is_client_error(),
        "one at a time"
    );
    // Reserved against handles from now on.
    let (st, v) = h
        .post(
            "com.atproto.identity.updateHandle",
            Some(&access),
            json!({"handle": "next.pds.example.com"}),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("HandleNotAvailable")),
        "{v}"
    );
    h.reload_hosts().await;
    // Health and /tls-check only.
    let (st, headers, _) = h
        .raw_at("next.pds.example.com", "GET", "/xrpc/_health")
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        headers["x-locus-instance"].to_str().unwrap(),
        crate::hostnames::instance_tag(&h.state)
    );
    let (st, _, _) = h
        .raw_at(
            "next.pds.example.com",
            "GET",
            "/xrpc/com.atproto.server.describeServer",
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "no identity on a candidate");
    assert!(crate::health::approved(&h.state, "next.pds.example.com").await);
    // Confirming needs the pre-checks (no HTTPS in tests) and re-auth.
    let (st, _) = h
        .admin_call(
            "POST",
            &ns("owner.hostname.confirm"),
            &owner,
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "re-auth");
    let (st, v) = h
        .admin_call(
            "POST",
            &ns("owner.hostname.confirm"),
            &owner,
            Some(json!({"password": "oto-password-1234", "skipChecks": true})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["message"].as_str().unwrap().contains("pre-check"), "{v}");
    let (_, v) = h
        .admin_call("GET", &ns("viewer.hostname.status"), &owner, None)
        .await;
    assert_eq!(v["candidate"], json!("next.pds.example.com"), "{v}");
    assert_eq!(v["prechecks"][1]["ok"], json!(false), "{v}");
    // Cancel: the candidate goes.
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
    assert!(!crate::health::approved(&h.state, "next.pds.example.com").await);
    assert!(
        !h.state
            .store
            .pds_host_reserved("next.pds.example.com")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn after_a_hostname_migration_the_old_host_keeps_serving_then_redirects() {
    let h = harness(false).await;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let (did, _) = h.user("quinn.pds.example.com").await;
    let old = "old.example.net";
    // This node is the new hostname: a migration from `old` is running.
    h.state
        .store
        .prepare_hostname_migration("m1", old, &h.state.hostname, "test")
        .await
        .unwrap();
    assert!(
        h.state
            .store
            .confirm_hostname_migration("m1", 0)
            .await
            .unwrap()
    );
    h.reload_hosts().await;
    assert_eq!(h.state.hosts().old_full, [old]);

    // The old host keeps full service, with its own service DID accepted.
    let (st, _, _) = h
        .raw_at(old, "GET", "/xrpc/com.atproto.server.describeServer")
        .await;
    assert_eq!(st, StatusCode::OK);
    let (_, _, doc) = h.raw_at(old, "GET", "/.well-known/did.json").await;
    let doc: Value = serde_json::from_slice(&doc).unwrap();
    assert_eq!(doc["id"], json!(format!("did:web:{old}")));
    assert_eq!(
        doc["service"][0]["serviceEndpoint"],
        json!(h.state.public_url)
    );
    assert!(h.state.accepts_aud(&format!("did:web:{old}")));
    assert!(h.state.accepts_aud(&h.state.service_did));
    assert!(!h.state.accepts_aud("did:web:elsewhere.example"));
    assert!(crate::health::approved(&h.state, old).await);
    let (_, _, pr) = h
        .raw_at(old, "GET", "/.well-known/oauth-protected-resource")
        .await;
    let pr: Value = serde_json::from_slice(&pr).unwrap();
    assert_eq!(
        pr["resource"],
        json!(format!("https://{old}")),
        "its own origin"
    );
    assert_eq!(
        pr["authorization_servers"],
        json!([crate::oauth::issuer(&h.state)])
    );
    let (_, _, asm) = h
        .raw(
            "GET",
            "/.well-known/oauth-authorization-server",
            None,
            vec![],
            &[],
        )
        .await;
    let asm: Value = serde_json::from_slice(&asm).unwrap();
    assert!(
        asm["protected_resources"]
            .as_array()
            .unwrap()
            .contains(&json!(format!("https://{old}"))),
        "{asm}"
    );
    // The old host and every PDS hostname stay reserved.
    assert!(h.state.store.pds_host_reserved(old).await.unwrap());

    // The job moves the account's PDS endpoint (still the old host's, as
    // for an account created before the migration) and announces it.
    {
        let mut plc = h.plc.lock().unwrap();
        let op = plc.log.get_mut(&did).unwrap().last_mut().unwrap();
        op["services"]["atproto_pds"]["endpoint"] = json!(format!("https://{old}"));
    }
    let before = h.outbox_kinds(&did).await.len();
    assert!(crate::hostnames::step(&h.state).await.unwrap());
    let endpoint = h.plc.lock().unwrap().last(&did).unwrap()["services"]["atproto_pds"]["endpoint"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(endpoint, h.state.public_url);
    assert_eq!(h.outbox_kinds(&did).await.len(), before + 1);
    assert_eq!(h.outbox_kinds(&did).await.last().unwrap(), "identity");
    // No stragglers and the window is over: the old host becomes former.
    assert!(crate::hostnames::step(&h.state).await.unwrap());
    let m = h.state.store.hostname_migrations().await.unwrap();
    assert_eq!(
        (m[0].state.as_str(), m[0].former_at.is_some()),
        ("done", true)
    );
    h.reload_hosts().await;
    assert_eq!(h.state.hosts().former, [old]);
    assert!(
        !h.state.accepts_aud(&format!("did:web:{old}")),
        "no document names it"
    );
    let (st, headers, _) = h
        .raw_at(old, "GET", "/xrpc/com.atproto.server.describeServer?x=1")
        .await;
    assert_eq!(st, StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        headers["location"].to_str().unwrap(),
        format!(
            "{}/xrpc/com.atproto.server.describeServer?x=1",
            h.state.public_url
        )
    );
    let (st, _, doc) = h.raw_at(old, "GET", "/.well-known/did.json").await;
    assert_eq!(st, StatusCode::OK, "its service document is served forever");
    let doc: Value = serde_json::from_slice(&doc).unwrap();
    assert_eq!(doc["id"], json!(format!("did:web:{old}")));
    assert!(
        crate::health::approved(&h.state, old).await,
        "its certificate never lapses"
    );
    let (_, v) = h
        .admin_call(
            "GET",
            &ns("viewer.hostname.status"),
            &format!("Bearer {}", h.operator("vic", "viewer").await),
            None,
        )
        .await;
    assert_eq!(v["formerHosts"], json!([old]), "{v}");
}

// ---------------------------------------------------------------- shard role

/// What a mock entryway received: (method, path, authorization, body).
type EntrywaySeen = Arc<Mutex<Vec<(String, String, String, String)>>>;

/// An entryway that records each request and answers `getSession` and
/// `createSession` with an account view, everything else with `{}`.
async fn mock_entryway() -> (String, EntrywaySeen) {
    let seen: EntrywaySeen = Arc::default();
    let app = Router::new()
        .route(
            "/xrpc/{nsid}",
            axum::routing::any(
                |State(seen): State<EntrywaySeen>,
                 Path(nsid): Path<String>,
                 method: axum::http::Method,
                 headers: axum::http::HeaderMap,
                 body: axum::body::Bytes| async move {
                    let auth = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_owned();
                    seen.lock().unwrap().push((
                        method.to_string(),
                        nsid.clone(),
                        auth,
                        String::from_utf8_lossy(&body).into_owned(),
                    ));
                    match nsid.as_str() {
                        "com.atproto.server.getSession" | "com.atproto.server.createSession" => {
                            axum::Json(json!({
                                "did": "did:plc:from-entryway",
                                "handle": "alice.test",
                                "email": "alice@example.com",
                                "emailConfirmed": true,
                            }))
                            .into_response()
                        }
                        "com.atproto.server.revokeAppPassword" => (
                            StatusCode::BAD_REQUEST,
                            axum::Json(json!({"error": "InvalidRequest", "message": "no such app password"})),
                        )
                            .into_response(),
                        _ => axum::Json(json!({})).into_response(),
                    }
                },
            ),
        )
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), seen)
}

/// A JWT signed ES256K with `key`.
fn es256k_jwt(key: &proto_blue_crypto::K256Keypair, typ: &str, claims: &Value) -> String {
    use base64::Engine;
    use proto_blue_crypto::Signer;
    let b64 = |b: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
    let input = format!(
        "{}.{}",
        b64(json!({"typ": typ, "alg": "ES256K"}).to_string().as_bytes()),
        b64(claims.to_string().as_bytes())
    );
    let sig = key.sign(input.as_bytes()).unwrap();
    format!("{input}.{}", b64(&sig))
}

#[tokio::test]
async fn a_shard_takes_entryway_tokens_and_creates_accounts_from_its_ops() {
    use proto_blue_crypto::K256Keypair;
    let token_key = K256Keypair::generate();
    let plc_key = K256Keypair::generate();
    let (url, seen) = mock_entryway().await;
    let ew_did = "did:web:entryway.test";
    let h = shard_harness(crate::shard::Entryway::for_test(
        &url,
        ew_did,
        &[token_key.did()],
        &[plc_key.did()],
    ))
    .await;

    // reserveSigningKey (unauthenticated), then createAccount with a
    // genesis op the entryway's PLC key signed.
    let (st, v) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.reserveSigningKey",
            None,
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let reserved = v["signingKey"].as_str().unwrap().to_owned();
    let op_for = |handle: &str, rotation: Vec<String>| locus_identity::plc::Operation {
        rotation_keys: rotation,
        verification_methods: [("atproto".to_owned(), reserved.clone())].into(),
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
    };
    let create = |did: String, op: SignedOp| json!({"handle": "alice.test", "did": did, "plcOp": op.to_json()});
    // Signed by another key, or not naming the entryway's: refused.
    let other = K256Keypair::generate();
    let forged = op_for("alice.test", vec![other.did(), plc_key.did()])
        .sign(&other)
        .unwrap();
    let (st, v) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.createAccount",
            None,
            Some(create(forged.genesis_did().unwrap(), forged)),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("IncompatibleDidDoc"))
    );
    let unnamed = op_for("alice.test", vec![other.did()])
        .sign(&other)
        .unwrap();
    let (st, v) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.createAccount",
            None,
            Some(create(unnamed.genesis_did().unwrap(), unnamed)),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("IncompatibleDidDoc"))
    );
    let op = op_for("alice.test", vec![plc_key.did()])
        .sign(&plc_key)
        .unwrap();
    let did = op.genesis_did().unwrap();
    let (st, v) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.createAccount",
            None,
            Some(create(did.clone(), op)),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["did"], did);
    let account = h.state.store.account_by_did(&did).await.unwrap().unwrap();
    assert_eq!((account.email, account.password_hash), (None, None));
    assert!(
        h.plc.lock().unwrap().last(&did).is_some(),
        "the shard submitted the op"
    );
    assert_eq!(
        h.outbox_kinds(&did).await,
        ["identity", "account", "commit", "sync"]
    );
    assert_eq!(
        h.state.signing_key(&did).await.unwrap().did(),
        reserved,
        "the reserved key signs the repo"
    );
    assert!(
        h.state
            .store
            .reserved_key(&reserved, &h.state.master_key)
            .await
            .unwrap()
            .is_none(),
        "the reservation is used up"
    );

    // Entryway access tokens work; others don't.
    let now = locus_core::time::now_ms() / 1000;
    let access = |key: &K256Keypair, aud: &str| {
        es256k_jwt(
            key,
            "at+jwt",
            &json!({"scope": "com.atproto.access", "aud": aud, "sub": did, "iat": now, "exp": now + 600}),
        )
    };
    let good = access(&token_key, "did:web:pds.example.com");
    let (st, v) = h
        .call(
            "POST",
            "/xrpc/com.atproto.repo.createRecord",
            Some(&good),
            Some(json!({"repo": did, "collection": "app.bsky.feed.post", "record": {
                "$type": "app.bsky.feed.post", "text": "on a shard", "createdAt": "2026-11-06T00:00:00Z"}})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    for bad in [
        access(&other, "did:web:pds.example.com"),
        access(&token_key, ew_did),
    ] {
        let (st, v) = h
            .call(
                "GET",
                "/xrpc/com.atproto.server.getSession",
                Some(&bad),
                None,
            )
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
        assert_eq!(v["error"], "InvalidToken");
    }

    // getSession goes to the entryway re-signed as the account.
    let (st, v) = h
        .call(
            "GET",
            "/xrpc/com.atproto.server.getSession",
            Some(&good),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["handle"], "alice.test");
    let (method, nsid, auth, _) = seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        (method.as_str(), nsid.as_str()),
        ("GET", "com.atproto.server.getSession")
    );
    let jwt = auth.strip_prefix("Bearer ").unwrap();
    let parsed =
        locus_auth::service_jwt::parse(jwt, Some("com.atproto.server.getSession"), now).unwrap();
    assert_eq!(
        (parsed.iss.as_str(), parsed.aud.as_str()),
        (did.as_str(), ew_did)
    );
    assert!(parsed.verifies_with(&reserved));

    // An entryway error comes back as it was.
    let (st, v) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.revokeAppPassword",
            Some(&good),
            Some(json!({"name": "x"})),
        )
        .await;
    assert_eq!(
        (st, v["message"].as_str()),
        (StatusCode::BAD_REQUEST, Some("no such app password"))
    );

    // createSession passes through with its body; refreshSession with the
    // client's token, after checking it's the entryway's.
    let (st, _) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.createSession",
            None,
            Some(json!({"identifier": "alice.test", "password": "pw"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        seen.lock()
            .unwrap()
            .last()
            .unwrap()
            .3
            .contains("alice.test")
    );
    let refresh = es256k_jwt(
        &token_key,
        "refresh+jwt",
        &json!({"scope": "com.atproto.refresh", "aud": ew_did, "sub": did, "jti": "r1", "iat": now, "exp": now + 600}),
    );
    let (st, _) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.refreshSession",
            Some(&refresh),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        seen.lock().unwrap().last().unwrap().2,
        format!("Bearer {refresh}")
    );
    let (st, _) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.refreshSession",
            Some(&good),
            None,
        )
        .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "an access token isn't a refresh token"
    );

    // Invites are the entryway's; its OAuth server too.
    let (st, v) = h
        .admin_call(
            "POST",
            "/xrpc/com.atproto.server.createInviteCode",
            &h.basic_admin(),
            Some(json!({"useCount": 1})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(
        v["message"],
        "Account invites are managed by the entryway service"
    );
    let (st, _) = h.call("POST", "/oauth/par", None, Some(json!({}))).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (_, meta) = h
        .call("GET", "/.well-known/oauth-protected-resource", None, None)
        .await;
    assert_eq!(meta["authorization_servers"], json!([url]));
    let (st, headers, _) = h
        .raw("GET", "/.well-known/change-password", None, vec![], &[])
        .await;
    assert!(st.is_redirection());
    assert_eq!(
        headers["location"],
        format!("{url}/.well-known/change-password").as_str()
    );

    // PLC credentials name the entryway's key.
    let (_, v) = h
        .call(
            "GET",
            "/xrpc/com.atproto.identity.getRecommendedDidCredentials",
            Some(&good),
            None,
        )
        .await;
    assert_eq!(v["rotationKeys"], json!([plc_key.did()]));

    // The entryway's handle callback: recorded once the document names it.
    let (st, v) = h
        .admin_call(
            "POST",
            "/xrpc/com.atproto.admin.updateAccountHandle",
            &h.basic_admin(),
            Some(json!({"did": did, "handle": "alice2.test"})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(v["message"], "Handle does not match DID doc");
    {
        let mut plc = h.plc.lock().unwrap();
        let last = plc.log.get_mut(&did).unwrap().last_mut().unwrap();
        last["alsoKnownAs"] = json!(["at://alice2.test"]);
    }
    let before = h.outbox_kinds(&did).await.len();
    let (st, v) = h
        .admin_call(
            "POST",
            "/xrpc/com.atproto.admin.updateAccountHandle",
            &h.basic_admin(),
            Some(json!({"did": did, "handle": "alice2.test"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        h.state
            .store
            .account_by_did(&did)
            .await
            .unwrap()
            .unwrap()
            .handle,
        "alice2.test"
    );
    assert_eq!(h.outbox_kinds(&did).await[before..], ["identity"]);
    assert_eq!(
        h.plc.lock().unwrap().log[&did].len(),
        1,
        "the shard didn't touch the PLC document"
    );
}

#[tokio::test]
async fn a_shard_changes_its_entryway_keys_with_keys_set() {
    use proto_blue_crypto::K256Keypair;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let (old_v, old_p) = (K256Keypair::generate(), K256Keypair::generate());
    let (url, _) = mock_entryway().await;
    let h = shard_harness(crate::shard::Entryway::for_test(
        &url,
        "did:web:entryway.test",
        &[old_v.did()],
        &[old_p.did()],
    ))
    .await;
    h.state
        .store
        .set_setting("entryway.plc_rotation_keys", json!([old_p.did()]), "test")
        .await
        .unwrap();
    let mut tx = h.state.store.begin().await.unwrap();
    tx.replace_shard_verify_keys(&[old_v.did()]).await.unwrap();
    tx.commit().await.unwrap();
    let owner = h.owner_with_password("sara", "sara-password-123").await;
    let reauth = json!({"password": "sara-password-123"});
    let set = ns("owner.keys.setSecret");
    let call = |body: Value| h.admin_call("POST", &set, &owner, Some(merge(body, &reauth)));

    // A new verify key, as hex: the old one stays accepted for a while.
    let new_v = K256Keypair::generate();
    let parsed = proto_blue_crypto::parse_did_key(&new_v.did()).unwrap();
    let (st, v) =
        call(json!({"name": "entryway-verify-key", "value": hex::encode(&parsed.key_bytes)})).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["accepted"], json!(new_v.did()));
    let keys = h.state.store.shard_verify_keys().await.unwrap();
    assert_eq!(keys.len(), 2);
    let old = keys.iter().find(|k| k.did_key == old_v.did()).unwrap();
    let left = old.expires_at.unwrap() - locus_core::time::now_ms();
    assert!(
        left > 0 && left <= crate::shard::VERIFY_KEY_OVERLAP_MS,
        "{left}"
    );
    assert_eq!(
        keys.iter()
            .find(|k| k.did_key == new_v.did())
            .unwrap()
            .expires_at,
        None
    );
    // --revoke-previous: only the new one.
    let newer = K256Keypair::generate();
    let (st, _) =
        call(json!({"name": "entryway-verify-key", "value": newer.did(), "revokePrevious": true}))
            .await;
    assert_eq!(st, StatusCode::OK);
    let keys = h.state.store.shard_verify_keys().await.unwrap();
    assert_eq!(
        keys.iter().map(|k| k.did_key.clone()).collect::<Vec<_>>(),
        [newer.did()]
    );

    // PLC rotation keys: earlier ones kept until --revoke-previous.
    let new_p = K256Keypair::generate();
    let (st, v) = call(json!({"name": "entryway-plc-rotation-key", "value": new_p.did()})).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["accepted"], json!([new_p.did(), old_p.did()]));
    let (st, v) = call(
        json!({"name": "entryway-plc-rotation-key", "value": new_p.did(), "revokePrevious": true}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["accepted"], json!([new_p.did()]));
    // Not a key: refused.
    let (st, _) = call(json!({"name": "entryway-verify-key", "value": "zz"})).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // The admin token is a secret.
    let (st, _) = call(json!({"name": "entryway-admin-token", "value": "tok-123"})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        h.state
            .store
            .get_secret(crate::shard::ADMIN_TOKEN_SECRET, &h.state.master_key)
            .await
            .unwrap()
            .unwrap()
            .as_slice(),
        b"tok-123"
    );
}

#[tokio::test]
async fn entryway_values_are_refused_outside_the_shard_role() {
    let h = harness(false).await;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let owner = h.owner_with_password("pia", "pia-password-1234").await;
    let body = merge(
        json!({"name": "entryway-admin-token", "value": "x"}),
        &json!({"password": "pia-password-1234"}),
    );
    let (st, v) = h
        .admin_call("POST", &ns("owner.keys.setSecret"), &owner, Some(body))
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    // And a standalone PDS reserves signing keys too, as the reference.
    let (st, v) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.reserveSigningKey",
            None,
            Some(json!({"did": "did:plc:abc"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (st, again) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.reserveSigningKey",
            None,
            Some(json!({"did": "did:plc:abc"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        v["signingKey"], again["signingKey"],
        "one reservation per DID"
    );
}

/// An account created on a shard from an op `plc_key` signed.
async fn shard_account(
    h: &Harness,
    plc_key: &proto_blue_crypto::K256Keypair,
    handle: &str,
) -> String {
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
        rotation_keys: vec![plc_key.did()],
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
    .sign(plc_key)
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
    assert_eq!(st, StatusCode::OK, "{v}");
    did
}

#[tokio::test]
async fn a_shard_takes_locus_callbacks_with_its_callback_token() {
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
    let did = shard_account(&h, &plc_key, "bea.test").await;
    let now = locus_core::time::now_ms() / 1000;

    // describeCapabilities says so.
    let (_, caps) = h.call("GET", &ns("describeCapabilities"), None, None).await;
    assert_eq!(
        (caps["role"].as_str(), caps["shardCallbacks"].as_bool()),
        (Some("shard"), Some(true))
    );

    // setCallbackToken: only a service JWT from the entryway's key.
    let set_token = ns("shard.setCallbackToken");
    let lxm = format!("{}.shard.setCallbackToken", locus_core::LOCUS_NS);
    let jwt = |key: &K256Keypair, iss: &str, aud: &str, lxm: &str| {
        locus_auth::service_jwt::create(key, iss, aud, Some(now + 60), Some(lxm), now).unwrap()
    };
    let cb = "a-shard-callback-token-of-enough-length-0001";
    let body = Some(json!({"token": cb}));
    for (bad, why) in [
        (
            jwt(
                &K256Keypair::generate(),
                ew_did,
                "did:web:pds.example.com",
                &lxm,
            ),
            "another key",
        ),
        (
            jwt(
                &token_key,
                "did:web:someone.else",
                "did:web:pds.example.com",
                &lxm,
            ),
            "another issuer",
        ),
        (jwt(&token_key, ew_did, ew_did, &lxm), "another audience"),
        (
            jwt(
                &token_key,
                ew_did,
                "did:web:pds.example.com",
                "com.atproto.server.getSession",
            ),
            "another method",
        ),
    ] {
        let (st, v) = h.call("POST", &set_token, Some(&bad), body.clone()).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{why}: {v}");
    }
    let (st, _) = h
        .admin_call("POST", &set_token, &h.basic_admin(), body.clone())
        .await;
    assert_ne!(st, StatusCode::OK, "the admin password doesn't install it");
    let good = jwt(&token_key, ew_did, "did:web:pds.example.com", &lxm);
    let (st, v) = h.call("POST", &set_token, Some(&good), body.clone()).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (st, _) = h.call("POST", &set_token, Some(&good), body.clone()).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "single use");

    // Callbacks need that token.
    let revoke = ns("shard.revokeSessions");
    let (st, _) = h
        .call(
            "POST",
            &revoke,
            Some("wrong-token"),
            Some(json!({"did": did, "sid": "s1"})),
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Revoking a session, then every token issued before a time.
    let access = |sid: &str, iat: i64| {
        es256k_jwt(
            &token_key,
            "at+jwt",
            &json!({"scope": "com.atproto.access", "aud": "did:web:pds.example.com",
                    "sub": did, "sid": sid, "iat": iat, "exp": iat + 600}),
        )
    };
    let (hr, dr) = (&h, did.as_str());
    let session = |t: String| async move {
        hr.call(
            "POST",
            "/xrpc/com.atproto.repo.createRecord",
            Some(&t),
            Some(
                json!({"repo": dr, "collection": "app.bsky.feed.post", "record": {
                "$type": "app.bsky.feed.post", "text": "hi", "createdAt": "2026-11-06T00:00:00Z"}}),
            ),
        )
        .await
    };
    assert_eq!(session(access("s1", now)).await.0, StatusCode::OK);
    let (st, _) = h
        .call(
            "POST",
            &revoke,
            Some(cb),
            Some(json!({"did": did, "sid": "s1"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, v) = session(access("s1", now)).await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("ExpiredToken"))
    );
    assert_eq!(session(access("s2", now)).await.0, StatusCode::OK);
    let cut = locus_core::time::iso8601((now + 2) * 1000);
    let (st, _) = h
        .call(
            "POST",
            &revoke,
            Some(cb),
            Some(json!({"did": did, "notBefore": cut})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(session(access("s2", now)).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(session(access("s3", now + 5)).await.0, StatusCode::OK);
    let (st, _) = h
        .call("POST", &revoke, Some(cb), Some(json!({"did": did})))
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "sid or notBefore");

    // The private-tier block comes from the entryway; the shard's own
    // operator API only shows it then.
    let (st, _) = h
        .call(
            "POST",
            &ns("shard.setPrivateTierBlock"),
            Some(cb),
            Some(json!({"did": did, "blocked": true})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(h.state.store.private_tier_blocked(&did).await.unwrap());
    let owner = h.owner_with_password("tess", "tess-password-123").await;
    let (st, v) = h
        .admin_call(
            "POST",
            &ns("moderator.account.setPrivateTierBlock"),
            &owner,
            Some(json!({"did": did, "blocked": false})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert!(h.state.store.private_tier_blocked(&did).await.unwrap());

    // setVerifyKeys replaces the accepted set and reloads the nodes.
    let generation = h.state.store.key_state().await.unwrap().generation;
    let next = K256Keypair::generate();
    let (st, v) = h
        .call(
            "POST",
            &ns("shard.setVerifyKeys"),
            Some(cb),
            Some(json!({"keys": [token_key.did(), next.did()]})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let mut keys: Vec<String> = h
        .state
        .store
        .shard_verify_keys()
        .await
        .unwrap()
        .into_iter()
        .map(|k| k.did_key)
        .collect();
    keys.sort();
    let mut want = vec![token_key.did(), next.did()];
    want.sort();
    assert_eq!(keys, want);
    assert!(h.state.store.key_state().await.unwrap().generation > generation);
    let (st, _) = h
        .call(
            "POST",
            &ns("shard.setVerifyKeys"),
            Some(cb),
            Some(json!({"keys": []})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn shard_callbacks_are_refused_outside_the_shard_role() {
    let h = harness(false).await;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let (st, _) = h
        .call(
            "POST",
            &ns("shard.revokeSessions"),
            Some("x"),
            Some(json!({"did": "did:plc:a", "sid": "s"})),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_IMPLEMENTED);
}

// ---------------------------------------------------------------- entryway role

/// Serves a node's router on `listener`.
fn serve_on(h: &Harness, listener: tokio::net::TcpListener) {
    let app = h
        .app
        .clone()
        .into_make_service_with_connect_info::<std::net::SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
}

/// A locus entryway and one locus shard behind it, served over TCP and
/// sharing a PLC directory: (entryway, shard, shard URL).
async fn entryway_and_shard() -> (Harness, Harness, String) {
    let (plc_url, plc) = mock_plc().await;
    let el = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sl = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let e_url = format!("http://{}", el.local_addr().unwrap());
    let s_url = format!("http://{}", sl.local_addr().unwrap());
    let token_key = proto_blue_crypto::K256Keypair::generate();
    let token_did = token_key.did();
    let e = harness_build(HarnessOpts {
        hostname: "entryway.example.com".into(),
        entryway_key: Some(token_key),
        plc: Some((plc_url.clone(), plc.clone())),
        public_url: Some(e_url.clone()),
        ..HarnessOpts::default()
    })
    .await;
    let plc_key = e.state.secrets.plc_keys.current_did().unwrap();
    let s = harness_build(HarnessOpts {
        entryway: Some(Arc::new(crate::shard::Entryway::for_test(
            &e_url,
            "did:web:entryway.example.com",
            &[token_did],
            &[plc_key],
        ))),
        plc: Some((plc_url, plc)),
        public_url: Some(s_url.clone()),
        ..HarnessOpts::default()
    })
    .await;
    serve_on(&e, el);
    serve_on(&s, sl);
    (e, s, s_url)
}

#[tokio::test]
async fn an_entryway_runs_accounts_on_a_locus_shard() {
    use locus_store::accounts::Status;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let (e, s, s_url) = entryway_and_shard().await;
    let owner = e.owner_with_password("ena", "ena-password-123").await;

    // No shard yet: no account can be made.
    let (st, _) = e
        .call(
            "POST",
            "/xrpc/com.atproto.server.createAccount",
            None,
            Some(json!({"handle": "ann.entryway.example.com", "email": "ann@example.com", "password": "ann-password"})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // shard add: a wrong admin password is refused, writing nothing.
    let (st, v) = e
        .admin_call(
            "POST",
            &ns("owner.shard.add"),
            &owner,
            Some(json!({"url": s_url, "adminPassword": "wrong", "name": "s1"})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert!(e.state.store.shards().await.unwrap().is_empty());
    let shard_pw = String::from_utf8(s.state.secrets.admin_password.to_vec()).unwrap();
    let (st, v) = e
        .admin_call(
            "POST",
            &ns("owner.shard.add"),
            &owner,
            Some(json!({"url": s_url, "adminPassword": shard_pw, "name": "s1"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        (v["locus"].as_bool(), v["did"].as_str()),
        (Some(true), Some("did:web:pds.example.com"))
    );
    assert!(
        s.state.store.callback_token_hash().await.unwrap().is_some(),
        "the shard got a callback token"
    );

    // An account: made on the shard, kept on the entryway.
    let (st, acct) = e
        .call(
            "POST",
            "/xrpc/com.atproto.server.createAccount",
            None,
            Some(json!({"handle": "ann.entryway.example.com", "email": "ann@example.com", "password": "ann-password"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{acct}");
    let did = acct["did"].as_str().unwrap().to_owned();
    let access = acct["accessJwt"].as_str().unwrap().to_owned();
    let on_e = e.state.store.account_by_did(&did).await.unwrap().unwrap();
    assert_eq!(on_e.email.as_deref(), Some("ann@example.com"));
    assert_eq!(
        e.state
            .store
            .account_shard_did(&did)
            .await
            .unwrap()
            .as_deref(),
        Some("did:web:pds.example.com")
    );
    let on_s = s.state.store.account_by_did(&did).await.unwrap().unwrap();
    assert_eq!((on_s.email, on_s.password_hash), (None, None));
    let doc_op = e.plc.lock().unwrap().last(&did).unwrap();
    assert_eq!(doc_op["services"]["atproto_pds"]["endpoint"], s_url);

    // The entryway's token works on the shard.
    let (st, v) = s
        .call(
            "POST",
            "/xrpc/com.atproto.repo.createRecord",
            Some(&access),
            Some(json!({"repo": did, "collection": "app.bsky.feed.post", "record": {
                "$type": "app.bsky.feed.post", "text": "via the entryway", "createdAt": "2026-11-06T00:00:00Z"}})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    // getSession on the shard: re-signed to the entryway, answered there.
    let (st, v) = s
        .call(
            "GET",
            "/xrpc/com.atproto.server.getSession",
            Some(&access),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        (v["did"].as_str(), v["email"].as_str()),
        (Some(did.as_str()), Some("ann@example.com"))
    );
    // Log in and refresh through the shard.
    let (st, sess) = s
        .call(
            "POST",
            "/xrpc/com.atproto.server.createSession",
            None,
            Some(json!({"identifier": "ann.entryway.example.com", "password": "ann-password"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{sess}");
    let refresh = sess["refreshJwt"].as_str().unwrap().to_owned();
    let (st, v) = s
        .call(
            "POST",
            "/xrpc/com.atproto.server.refreshSession",
            Some(&refresh),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    // Repo calls reaching the entryway go to the shard.
    let (st, v) = e
        .call(
            "GET",
            &format!("/xrpc/com.atproto.repo.listRecords?repo={did}&collection=app.bsky.feed.post"),
            None,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["records"][0]["value"]["text"], "via the entryway");

    // Deactivate and reactivate through the shard: the entryway decides,
    // the shard is told.
    let (st, v) = s
        .call(
            "POST",
            "/xrpc/com.atproto.server.deactivateAccount",
            Some(&access),
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let status = |h: &Harness| {
        let (store, did) = (h.state.store.clone(), did.clone());
        async move { store.account_by_did(&did).await.unwrap().unwrap().status }
    };
    assert_eq!(status(&e).await, Status::Deactivated);
    assert_eq!(status(&s).await, Status::Deactivated);
    let (st, v) = s
        .call(
            "POST",
            "/xrpc/com.atproto.server.activateAccount",
            Some(&access),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        (status(&e).await, status(&s).await),
        (Status::Active, Status::Active)
    );

    // A handle change: the entryway updates PLC, the shard records it.
    let (st, v) = s
        .call(
            "POST",
            "/xrpc/com.atproto.identity.updateHandle",
            Some(&access),
            Some(json!({"handle": "ann2.entryway.example.com"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        s.state
            .store
            .account_by_did(&did)
            .await
            .unwrap()
            .unwrap()
            .handle,
        "ann2.entryway.example.com"
    );

    // A takedown on the entryway reaches the shard.
    let (st, v) = e
        .admin_call(
            "POST",
            "/xrpc/com.atproto.admin.updateSubjectStatus",
            &e.basic_admin(),
            Some(
                json!({"subject": {"$type": "com.atproto.admin.defs#repoRef", "did": did},
                        "takedown": {"applied": true, "ref": "td-1"}}),
            ),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(status(&s).await, Status::Takendown);

    // Shards are listed with their accounts, weighted and drained.
    let (_, v) = e
        .admin_call("GET", &ns("owner.shard.list"), &owner, None)
        .await;
    assert_eq!(v["shards"][0]["accounts"], 1);
    let (st, v) = e
        .admin_call(
            "POST",
            &ns("owner.shard.update"),
            &owner,
            Some(json!({"name": "s1", "draining": true})),
        )
        .await;
    assert_eq!((st, v["draining"].as_bool()), (StatusCode::OK, Some(true)));
    let (st, _) = e
        .call(
            "POST",
            "/xrpc/com.atproto.server.createAccount",
            None,
            Some(json!({"handle": "bo2.entryway.example.com", "email": "bo@example.com", "password": "bo-password-1"})),
        )
        .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "a draining shard takes no accounts"
    );
}

#[tokio::test]
async fn entryway_methods_are_refused_in_other_roles() {
    let h = harness(false).await;
    let owner = h.owner_with_password("eli", "eli-password-123").await;
    let (st, _) = h
        .admin_call(
            "GET",
            &format!("/xrpc/{}.owner.shard.list", locus_core::LOCUS_NS),
            &owner,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_IMPLEMENTED);
    let (st, _) = h
        .call(
            "GET",
            "/xrpc/com.atproto.temp.dereferenceScope?scope=ref:x",
            None,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_IMPLEMENTED);
    h.console_operator("vic", "viewer", "vic-password-1234")
        .await;
    let b = Browser::default();
    h.console_sign_in(&b, "vic", "vic-password-1234").await;
    let (st, _, page) = b.get(&h, "/admin/shards").await;
    assert_eq!(st, StatusCode::OK);
    assert!(page.contains("this node isn"), "{page}");
}

#[tokio::test]
async fn an_entryway_pushes_revocations_blocks_and_key_changes_to_its_shards() {
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let (e, s, s_url) = entryway_and_shard().await;
    let owner = e.owner_with_password("ena", "ena-password-123").await;
    let reauth = json!({"password": "ena-password-123"});
    let shard_pw = String::from_utf8(s.state.secrets.admin_password.to_vec()).unwrap();
    let (st, v) = e
        .admin_call(
            "POST",
            &ns("owner.shard.add"),
            &owner,
            Some(json!({"url": s_url, "adminPassword": shard_pw, "name": "s1"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    // The add queued the current keys; deliver them.
    crate::entryway::deliver_pushes(&e.state).await.unwrap();
    let (_, acct) = e
        .call(
            "POST",
            "/xrpc/com.atproto.server.createAccount",
            None,
            Some(json!({"handle": "cal.entryway.example.com", "email": "cal@example.com", "password": "cal-password"})),
        )
        .await;
    let did = acct["did"].as_str().unwrap().to_owned();
    let first = acct["accessJwt"].as_str().unwrap().to_owned();
    let (_, sess) = e
        .call(
            "POST",
            "/xrpc/com.atproto.server.createSession",
            None,
            Some(json!({"identifier": did, "password": "cal-password"})),
        )
        .await;
    let access = sess["accessJwt"].as_str().unwrap().to_owned();
    let refresh = sess["refreshJwt"].as_str().unwrap().to_owned();
    let sr = &s;
    let works = |t: String| async move {
        sr.call(
            "GET",
            "/xrpc/com.atproto.server.checkAccountStatus",
            Some(&t),
            None,
        )
        .await
        .0
    };
    assert_eq!(works(access.clone()).await, StatusCode::OK);

    // Logout: the shard refuses that session's tokens once the push lands.
    let (st, _) = e
        .call(
            "POST",
            "/xrpc/com.atproto.server.deleteSession",
            Some(&refresh),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        works(access.clone()).await,
        StatusCode::OK,
        "not yet delivered"
    );
    assert_eq!(crate::entryway::deliver_pushes(&e.state).await.unwrap(), 1);
    assert_eq!(works(access.clone()).await, StatusCode::BAD_REQUEST);
    assert_eq!(
        works(first.clone()).await,
        StatusCode::OK,
        "another session"
    );

    // A password change refuses every earlier token.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let (st, v) = e
        .admin_call(
            "POST",
            "/xrpc/com.atproto.admin.updateAccountPassword",
            &e.basic_admin(),
            Some(json!({"did": did, "password": "cal-password-2"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    crate::entryway::deliver_pushes(&e.state).await.unwrap();
    assert_eq!(works(first.clone()).await, StatusCode::BAD_REQUEST);

    // Private-tier blocks: the latest queued one wins (FIFO, superseded).
    for blocked in [true, false, true] {
        let (st, v) = e
            .admin_call(
                "POST",
                &ns("moderator.account.setPrivateTierBlock"),
                &owner,
                Some(json!({"did": did, "blocked": blocked})),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
    }
    let backlog = e.state.store.shard_backlog().await.unwrap();
    assert_eq!(backlog, [("s1".to_owned(), 1, None)]);
    // The console's shards page lists it with its queue.
    e.console_operator("cora", "viewer", "cora-password-123")
        .await;
    let b = Browser::default();
    e.console_sign_in(&b, "cora", "cora-password-123").await;
    let (st, _, page) = b.get(&e, "/admin/shards").await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        page.contains(&s_url) && page.contains("taking accounts"),
        "{page}"
    );
    crate::entryway::deliver_pushes(&e.state).await.unwrap();
    assert!(s.state.store.private_tier_blocked(&did).await.unwrap());

    // A token-key rotation: the shard takes {old, new}, then signing
    // switches; two hours later the shard keeps only the new key.
    let (st, v) = e
        .admin_call(
            "POST",
            &ns("owner.keys.rotateEntrywayKey"),
            &owner,
            Some(reauth.clone()),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (old, new) = (
        v["oldKey"].as_str().unwrap().to_owned(),
        v["newKey"].as_str().unwrap().to_owned(),
    );
    crate::entryway::rotation_step(&e.state).await.unwrap();
    assert!(
        e.state
            .store
            .key_rotation()
            .await
            .unwrap()
            .unwrap()
            .switched_at
            .is_none(),
        "not acknowledged yet"
    );
    crate::entryway::deliver_pushes(&e.state).await.unwrap();
    let mut keys: Vec<String> = s
        .state
        .store
        .shard_verify_keys()
        .await
        .unwrap()
        .into_iter()
        .map(|k| k.did_key)
        .collect();
    keys.sort();
    let mut both = vec![old.clone(), new.clone()];
    both.sort();
    assert_eq!(keys, both);
    crate::entryway::rotation_step(&e.state).await.unwrap();
    assert!(
        e.state
            .store
            .key_rotation()
            .await
            .unwrap()
            .unwrap()
            .switched_at
            .is_some()
    );
    assert_eq!(
        crate::entryway::token_key(&e.state).await.unwrap().did(),
        new
    );
    let Some(pool) = sqlite_pool(&e.state.store) else {
        return; // SQLite-only: it reads or sets rows directly
    };
    sqlx::query("UPDATE entryway_key_rotation SET switched_at = switched_at - 7300000")
        .execute(pool)
        .await
        .unwrap();
    crate::entryway::rotation_step(&e.state).await.unwrap();
    crate::entryway::deliver_pushes(&e.state).await.unwrap();
    let keys: Vec<String> = s
        .state
        .store
        .shard_verify_keys()
        .await
        .unwrap()
        .into_iter()
        .map(|k| k.did_key)
        .collect();
    assert_eq!(keys, [new.clone()]);
    assert!(
        e.state
            .store
            .key_rotation()
            .await
            .unwrap()
            .unwrap()
            .finished_at
            .is_some()
    );

    // A new PLC rotation key becomes current only once the shard is
    // confirmed (besides the key backup).
    let set = ns("owner.keys.setPlcRotationKey");
    let before = e.state.secrets.plc_keys.current_did().unwrap();
    let (st, v) = e
        .admin_call(
            "POST",
            &set,
            &owner,
            Some(merge(json!({"generate": true}), &reauth)),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let pending = v["didKey"].as_str().unwrap().to_owned();
    e.state
        .store
        .record_key_backup(locus_core::time::now_ms() + 1000, "test")
        .await
        .unwrap();
    let current = || async {
        e.state
            .store
            .plc_key_rows()
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.role == "current")
            .unwrap()
            .did_key
    };
    assert_eq!(current().await, before, "the shard isn't confirmed");
    let (st, v) = e
        .admin_call(
            "POST",
            &set,
            &owner,
            Some(merge(json!({"discardPending": true}), &reauth)),
        )
        .await;
    assert_eq!(
        st,
        StatusCode::OK,
        "discardable before any confirmation: {v}"
    );
    let (_, v) = e
        .admin_call(
            "POST",
            &set,
            &owner,
            Some(merge(json!({"generate": true}), &reauth)),
        )
        .await;
    let pending = {
        let _ = pending;
        v["didKey"].as_str().unwrap().to_owned()
    };
    e.state
        .store
        .record_key_backup(locus_core::time::now_ms() + 1000, "test")
        .await
        .unwrap();
    let (st, v) = e
        .admin_call(
            "POST",
            &set,
            &owner,
            Some(merge(json!({"confirm": "s1"}), &reauth)),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["role"], "current");
    assert_eq!(current().await, pending);
}

#[tokio::test]
async fn the_orphan_sweep_queues_only_untracked_keys_under_locus_prefixes() {
    let h = harness(false).await;
    if h.state.blobs.name() != "disk" {
        return; // the S3 test bucket is shared with other tests
    }
    let (_did, access) = h.user("swe.pds.example.com").await;
    let uploaded = h.upload(&access, b"tracked bytes", "image/png").await;
    let cid = uploaded["ref"]["$link"].as_str().unwrap().to_owned();
    let write = |key: &'static str| {
        let blobs = h.state.blobs.clone();
        async move {
            let body = futures::stream::iter([Ok::<_, std::io::Error>(
                axum::body::Bytes::from_static(b"x"),
            )]);
            blobs.write_stream(key, body, 10).await.unwrap();
        }
    };
    write("blobs/did:plc:gone/bafkreiorphan.0001").await;
    write("tmp/0002").await;
    write("other/not-ours").await;
    // Too young: nothing.
    assert_eq!(
        locus_blob::run_orphan_sweep(&h.state.store, &h.state.blobs, 3_600_000)
            .await
            .unwrap(),
        0
    );
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let n = locus_blob::run_orphan_sweep(&h.state.store, &h.state.blobs, 0)
        .await
        .unwrap();
    assert_eq!(n, 2, "the two orphans under locus prefixes");
    let Some(pool) = sqlite_pool(&h.state.store) else {
        return; // SQLite-only: it reads or sets rows directly
    };
    let queued: Vec<(String,)> =
        sqlx::query_as("SELECT storage_key FROM blob_delete ORDER BY storage_key")
            .fetch_all(pool)
            .await
            .unwrap();
    let keys: Vec<&str> = queued.iter().map(|(k,)| k.as_str()).collect();
    assert_eq!(keys, ["blobs/did:plc:gone/bafkreiorphan.0001", "tmp/0002"]);
    assert!(
        !keys.iter().any(|k| k.contains(&cid)),
        "the upload is tracked"
    );
    // Already queued: not again.
    assert_eq!(
        locus_blob::run_orphan_sweep(&h.state.store, &h.state.blobs, 0)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn jobs_are_paused_and_run_through_the_admin_api() {
    let h = harness(false).await;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let owner = h.owner_with_password("jo", "jo-password-12345").await;
    h.state
        .store
        .record_job_run("blob-gc", None, None)
        .await
        .unwrap();
    let (st, _) = h
        .admin_call(
            "POST",
            &ns("admin.jobs.setPaused"),
            &owner,
            Some(json!({"name": "blob-gc", "paused": true})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (_, v) = h
        .admin_call("GET", &ns("viewer.listJobs"), &owner, None)
        .await;
    assert_eq!(v["jobs"][0]["paused"], true);
    let (st, _) = h
        .admin_call(
            "POST",
            &ns("admin.jobs.runNow"),
            &owner,
            Some(json!({"name": "blob-gc"})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "paused jobs don't run now");
    let (st, _) = h
        .admin_call(
            "POST",
            &ns("admin.jobs.setPaused"),
            &owner,
            Some(json!({"name": "blob-gc", "paused": false})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = h
        .admin_call(
            "POST",
            &ns("admin.jobs.runNow"),
            &owner,
            Some(json!({"name": "blob-gc"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (_, v) = h
        .admin_call("GET", &ns("viewer.listJobs"), &owner, None)
        .await;
    assert_eq!(v["jobs"][0]["runRequested"], true);
}

#[tokio::test]
async fn repair_tools_fix_the_index_the_tree_and_the_identity() {
    let h = harness(false).await;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let owner = h.owner_with_password("ren", "ren-password-123").await;
    let (did, access) = h.user("rep.pds.example.com").await;
    let mut rkeys = Vec::new();
    for i in 0..5 {
        let (st, v) = h
            .call(
                "POST",
                "/xrpc/com.atproto.repo.createRecord",
                Some(&access),
                Some(json!({"repo": did, "collection": "app.bsky.feed.post", "record": post_record(&format!("post {i}"))})),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        rkeys.push(
            v["uri"]
                .as_str()
                .unwrap()
                .rsplit('/')
                .next()
                .unwrap()
                .to_owned(),
        );
    }
    let Some(pool) = sqlite_pool(&h.state.store) else {
        return; // SQLite-only: it reads or sets rows directly
    };
    let count = |pool: &sqlx::SqlitePool, did: String| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM record WHERE did = ?1")
                .bind(did)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };

    // The index loses a row and gains a bogus one: repair repo fixes both.
    sqlx::query("DELETE FROM record WHERE did = ?1 AND rkey = ?2")
        .bind(&did)
        .bind(&rkeys[0])
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO record (did, collection, rkey, cid, rev, indexed_at) \
         VALUES (?1, 'app.bsky.feed.post', 'zzzbogus', 'bafyreibogus', 'x', 0)",
    )
    .bind(&did)
    .execute(pool)
    .await
    .unwrap();
    let (st, v) = h
        .admin_call(
            "POST",
            &ns("admin.repo.repair"),
            &owner,
            Some(json!({"did": did})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        (v["added"].as_u64(), v["removed"].as_u64()),
        (Some(1), Some(1))
    );
    assert_eq!(count(pool, did.clone()).await, 5);

    // The tree's root node is lost: repair refuses, rebuild makes a new
    // tree and commit from the index, with #sync.
    let root = h.state.store.repo_root_of(&did).await.unwrap().unwrap();
    sqlx::query("DELETE FROM repo_block WHERE did = ?1 AND cid = ?2")
        .bind(&did)
        .bind(&root.data_cid)
        .execute(pool)
        .await
        .unwrap();
    let (st, v) = h
        .admin_call(
            "POST",
            &ns("admin.repo.repair"),
            &owner,
            Some(json!({"did": did})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    let before = h.outbox_kinds(&did).await.len();
    let (st, v) = h
        .admin_call(
            "POST",
            &ns("admin.repo.rebuild"),
            &owner,
            Some(json!({"did": did})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["records"], 5);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let rebuilt = h.state.store.repo_root_of(&did).await.unwrap().unwrap();
    assert_ne!(rebuilt.commit_cid, root.commit_cid);
    assert!(rebuilt.rev > root.rev);
    let old_refs: i64 =
        sqlx::query_scalar("SELECT ref_count FROM repo_block WHERE did = ?1 AND cid = ?2")
            .bind(&did)
            .bind(&root.commit_cid)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(old_refs, 0, "the old commit is left for GC");
    assert!(h.outbox_kinds(&did).await[before..].contains(&"sync".to_owned()));
    let (st, v) = h
        .admin_call(
            "POST",
            &ns("admin.repo.repair"),
            &owner,
            Some(json!({"did": did})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "the rebuilt tree is whole: {v}");
    assert_eq!(
        (
            v["added"].as_u64(),
            v["removed"].as_u64(),
            v["updated"].as_u64()
        ),
        (Some(0), Some(0), Some(0))
    );
    let (st, car) = {
        let (st, _, body) = h
            .raw(
                "GET",
                &format!("/xrpc/com.atproto.sync.getRepo?did={did}"),
                None,
                vec![],
                &[],
            )
            .await;
        (st, body)
    };
    assert_eq!(st, StatusCode::OK);
    let (_, blocks) = proto_blue_repo::car::read_car(&car).unwrap();
    assert!(blocks.iter().count() >= 7, "commit, tree and 5 records");
    // And it takes writes again.
    let (st, _) = h
        .call(
            "POST",
            "/xrpc/com.atproto.repo.createRecord",
            Some(&access),
            Some(json!({"repo": did, "collection": "app.bsky.feed.post", "record": post_record("after")})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);

    // The PLC document drifted: repair identity sets it back.
    {
        let mut plc = h.plc.lock().unwrap();
        let last = plc.log.get_mut(&did).unwrap().last_mut().unwrap();
        last["alsoKnownAs"] = json!(["at://someone-else.example"]);
        last["services"]["atproto_pds"]["endpoint"] = json!("https://elsewhere.example");
    }
    let before = h.outbox_kinds(&did).await.len();
    let (st, v) = h
        .admin_call(
            "POST",
            &ns("admin.identity.repair"),
            &owner,
            Some(json!({"did": did})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["plcUpdated"], true);
    let op = h.plc.lock().unwrap().last(&did).unwrap();
    assert_eq!(op["alsoKnownAs"][0], "at://rep.pds.example.com");
    assert_eq!(
        op["services"]["atproto_pds"]["endpoint"],
        "https://pds.example.com"
    );
    assert_eq!(h.outbox_kinds(&did).await[before..], ["identity"]);
    // Nothing to fix the second time.
    let (_, v) = h
        .admin_call(
            "POST",
            &ns("admin.identity.repair"),
            &owner,
            Some(json!({"did": did})),
        )
        .await;
    assert_eq!(v["plcUpdated"], false);
}

#[tokio::test]
async fn sequencer_recovery_replays_the_event_log_onto_repos() {
    let h = harness(false).await;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let owner = h.owner_with_password("seq", "seq-password-123").await;
    let lease = h
        .state
        .store
        .try_acquire_lease("sequencer", "test", std::time::Duration::from_secs(60))
        .await
        .unwrap()
        .unwrap();
    let (did, access) = h.user("rec.pds.example.com").await;
    let post = |text: &'static str| {
        let (h, did, access) = (&h, did.clone(), access.clone());
        async move {
            let (st, v) = h
                .call(
                    "POST",
                    "/xrpc/com.atproto.repo.createRecord",
                    Some(&access),
                    Some(json!({"repo": did, "collection": "app.bsky.feed.post", "record": post_record(text)})),
                )
                .await;
            assert_eq!(st, StatusCode::OK, "{v}");
        }
    };
    post("one").await;
    let early = h.state.store.repo_root_of(&did).await.unwrap().unwrap();
    post("two").await;
    post("three").await;
    locus_sync::sequence_once(&h.state.store, &lease, 500, 0)
        .await
        .unwrap();
    let latest = h.state.store.repo_root_of(&did).await.unwrap().unwrap();
    let Some(pool) = sqlite_pool(&h.state.store) else {
        return; // SQLite-only: it reads or sets rows directly
    };

    // Data loss: the repo back at its first post, later blocks and index
    // rows gone (the event log kept).
    let roll_back = || async {
        sqlx::query("DELETE FROM repo_block WHERE did = ?1 AND rev_introduced > ?2")
            .bind(&did)
            .bind(&early.rev)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM record WHERE did = ?1 AND rev > ?2")
            .bind(&did)
            .bind(&early.rev)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("UPDATE repo_root SET commit_cid = ?2, rev = ?3, data_cid = ?4 WHERE did = ?1")
            .bind(&did)
            .bind(&early.commit_cid)
            .bind(&early.rev)
            .bind(&early.data_cid)
            .execute(pool)
            .await
            .unwrap();
    };
    roll_back().await;
    let count = || async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM record WHERE did = ?1")
            .bind(&did)
            .fetch_one(pool)
            .await
            .unwrap()
    };
    assert_eq!(count().await, 1);
    let (st, v) = h
        .admin_call(
            "POST",
            &ns("admin.repo.recoverFromSequencer"),
            &owner,
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["recovered"], json!([did]), "{v}");
    let now = h.state.store.repo_root_of(&did).await.unwrap().unwrap();
    assert_eq!(
        now.commit_cid, latest.commit_cid,
        "the newest commit adopted"
    );
    assert_eq!(count().await, 3, "the index follows");
    // Run again: up to date.
    let (_, v) = h
        .admin_call(
            "POST",
            &ns("admin.repo.recoverFromSequencer"),
            &owner,
            Some(json!({})),
        )
        .await;
    assert_eq!(v["upToDate"], 1);

    // When the log's commits and tree nodes are gone too (its record blocks
    // kept), recovery can't adopt anything: the account is listed, and
    // repair repos --from-sequencer replays the log's record changes into
    // the index and rebuilds the repository from it.
    roll_back().await;
    let rows: Vec<(i64, Vec<u8>)> =
        sqlx::query_as("SELECT seq, payload FROM seq_event WHERE did = ?1 AND kind = 'commit'")
            .bind(&did)
            .fetch_all(pool)
            .await
            .unwrap();
    for (seq, payload) in rows {
        let mut v: proto_blue_lex_data::LexValue = proto_blue_lex_cbor::decode(&payload).unwrap();
        if let proto_blue_lex_data::LexValue::Map(m) = &mut v
            && m.get("rev")
                .and_then(proto_blue_lex_data::LexValue::as_str)
                .is_some_and(|r| r > early.rev.as_str())
        {
            let ops: Vec<String> = m
                .get("ops")
                .and_then(proto_blue_lex_data::LexValue::as_array)
                .into_iter()
                .flatten()
                .filter_map(|o| {
                    o.as_map()?
                        .get("cid")?
                        .as_cid()
                        .map(|c| c.to_string_base32())
                })
                .collect();
            let Some(proto_blue_lex_data::LexValue::Bytes(car)) = m.get("blocks") else {
                unreachable!()
            };
            let (_, blocks) = proto_blue_repo::car::read_car(car).unwrap();
            let mut kept = locus_repo::car::header(None);
            for (cid, bytes) in blocks.iter() {
                if ops.contains(&cid.to_string_base32()) {
                    kept.extend(locus_repo::car::block(cid, bytes));
                }
            }
            m.insert("blocks".into(), proto_blue_lex_data::LexValue::Bytes(kept));
        }
        sqlx::query("UPDATE seq_event SET payload = ?2 WHERE seq = ?1")
            .bind(seq)
            .bind(proto_blue_lex_cbor::encode(&v).unwrap())
            .execute(pool)
            .await
            .unwrap();
    }
    let (_, v) = h
        .admin_call(
            "POST",
            &ns("admin.repo.recoverFromSequencer"),
            &owner,
            Some(json!({})),
        )
        .await;
    assert_eq!(v["failed"][0]["did"], did, "{v}");
    let (st, v) = h
        .admin_call(
            "POST",
            &ns("admin.repo.repairFromSequencer"),
            &owner,
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["fixed"], json!([did]));
    let rebuilt = h.state.store.repo_root_of(&did).await.unwrap().unwrap();
    assert_ne!(rebuilt.commit_cid, early.commit_cid, "rebuilt");
    assert_eq!(
        count().await,
        3,
        "the log's changes replayed into the index"
    );
    let (st, v) = h
        .call(
            "GET",
            &format!("/xrpc/com.atproto.sync.getRepo?did={did}"),
            None,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(
        h.state
            .store
            .recoveries()
            .await
            .unwrap()
            .iter()
            .all(|r| r.fixed)
    );
    // rotate-account --recovery takes the recovered accounts.
    let (st, v) = h
        .admin_call(
            "POST",
            &ns("admin.keys.rotateAccountKeys"),
            &owner,
            Some(json!({"recovery": true})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["results"][0]["did"], did);
}

/// With Redis (LOCUS_TEST_REDIS_URL), rate-limit counters and replay
/// windows live there, shared by every node using it.
#[tokio::test]
async fn redis_holds_rate_limits_and_replay_windows() {
    let Ok(url) = std::env::var("LOCUS_TEST_REDIS_URL") else {
        return;
    };
    let kv = crate::scratch::Scratch::connect(&url).await.unwrap();
    let h = harness_build(HarnessOpts {
        kv: kv.clone(),
        ..HarnessOpts::default()
    })
    .await;
    let other = harness_build(HarnessOpts {
        kv,
        ..HarnessOpts::default()
    })
    .await;
    let run = locus_core::time::now_ms().to_string();
    // Replay windows: once, across both nodes.
    assert!(h.state.unique("Test", &run, 60_000).await.unwrap());
    assert!(!other.state.unique("Test", &run, 60_000).await.unwrap());
    assert!(other.state.unique("Other", &run, 60_000).await.unwrap());
    // A window ends.
    let short = format!("{run}-short");
    assert!(h.state.unique("Test", &short, 50).await.unwrap());
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    assert!(other.state.unique("Test", &short, 60_000).await.unwrap());
    // The database's table isn't used.
    let Some(pool) = sqlite_pool(&h.state.store) else {
        return; // SQLite-only: it reads or sets rows directly
    };
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oauth_replay")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);

    // Rate limits: a lowered global bucket, shared. Each node's requests
    // count against the same client IP.
    for n in [&h, &other] {
        n.state
            .settings
            .set("rate_limits.global_points", json!(3), "test")
            .await
            .unwrap();
    }
    let path = "/xrpc/com.atproto.server.describeServer".to_owned();
    // A client address of its own each run (the counters outlive a run).
    let now = locus_core::time::now_ms();
    let ip: std::net::SocketAddr = format!(
        "10.{}.{}.{}:1",
        (now >> 16) & 255,
        (now >> 8) & 255,
        now & 255
    )
    .parse()
    .unwrap();
    let call = |n: &Harness| {
        let req = Request::builder()
            .uri(path.clone())
            .header("host", n.state.hostname.clone())
            .extension(axum::extract::ConnectInfo(ip))
            .body(Body::empty())
            .unwrap();
        let app = n.app.clone();
        async move { app.oneshot(req).await.unwrap() }
    };
    let r = call(&h).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()["ratelimit-remaining"], "2");
    assert_eq!(call(&other).await.headers()["ratelimit-remaining"], "1");
    assert_eq!(call(&h).await.status(), StatusCode::OK);
    let r = call(&other).await;
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(r.headers().contains_key("retry-after"));
}

#[tokio::test]
async fn metrics_are_served_to_allowed_addresses_or_the_token() {
    let h = harness(false).await;
    let get = |peer: &'static str, token: Option<&'static str>| {
        let h = &h;
        async move {
            let mut req = Request::builder()
                .uri("/metrics")
                .header("host", h.state.hostname.clone());
            if let Some(t) = token {
                req = req.header("authorization", format!("Bearer {t}"));
            }
            let mut req = req.body(Body::empty()).unwrap();
            let addr: std::net::SocketAddr = format!("{peer}:40000").parse().unwrap();
            req.extensions_mut()
                .insert(axum::extract::ConnectInfo(addr));
            let res = h.app.clone().oneshot(req).await.unwrap();
            let status = res.status();
            let bytes = res.into_body().collect().await.unwrap().to_bytes();
            (status, String::from_utf8(bytes.to_vec()).unwrap())
        }
    };
    let (did, access) = h.user("met.pds.example.com").await;
    let (st, _) = h
        .call(
            "POST",
            "/xrpc/com.atproto.repo.createRecord",
            Some(&access),
            Some(json!({"repo": did, "collection": "app.bsky.feed.post", "record": post_record("m")})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    h.call("GET", "/xrpc/not..valid", None, None).await;

    let (st, body) = get("127.0.0.1", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        body.contains(
            r#"locus_http_requests_total{nsid="com.atproto.repo.createRecord",status="200"}"#
        ),
        "{body}"
    );
    assert!(body.contains(
        r#"locus_http_request_seconds_bucket{nsid="com.atproto.repo.createRecord",le="+Inf"}"#
    ));
    assert!(body.contains(r#"locus_http_requests_total{nsid="invalid","#));
    assert!(body.contains("locus_repo_commit_seconds_count"));
    assert!(body.contains(r#"locus_db_pool_connections{state="idle"}"#));
    assert!(body.contains("# TYPE locus_firehose_subscribers gauge"));

    // Elsewhere: refused, unless with the token.
    assert_eq!(get("203.0.113.9", None).await.0, StatusCode::FORBIDDEN);
    h.state
        .settings
        .set("metrics.token", json!("metrics-token-123"), "test")
        .await
        .unwrap();
    assert_eq!(
        get("203.0.113.9", Some("wrong")).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get("203.0.113.9", Some("metrics-token-123")).await.0,
        StatusCode::OK
    );
    h.state
        .settings
        .set("metrics.enabled", json!(false), "test")
        .await
        .unwrap();
    assert_eq!(get("127.0.0.1", None).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn answers_match_the_reference_where_the_conformance_run_found_them_apart() {
    let h = harness(false).await;
    // describeServer: the reference's fields.
    let (_, v) = h
        .call("GET", "/xrpc/com.atproto.server.describeServer", None, None)
        .await;
    assert_eq!(v["contact"], json!({}));
    assert!(v["blobUploadLimit"].is_i64());
    assert!(v.get("phoneVerificationRequired").is_none());
    // A query the extractor refuses is an XRPC error.
    let (st, v) = h
        .call("GET", "/xrpc/com.atproto.server.getServiceAuth", None, None)
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, v2) = h
        .call("GET", "/xrpc/com.atproto.sync.getRepoStatus", None, None)
        .await;
    assert_eq!(
        (st, v2["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("InvalidRequest")),
        "{v2}"
    );
    let _ = v;
    // Handle syntax on a handle-format input: InvalidRequest.
    let (st, v) = h
        .call(
            "GET",
            "/xrpc/com.atproto.identity.resolveHandle?handle=not%20a%20handle",
            None,
            None,
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("InvalidRequest"))
    );
    let (did, access) = h.user("conf.pds.example.com").await;
    let (_, v) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.createAccount",
            None,
            Some(json!({"handle": "conf.pds.example.com", "email": "other@example.com", "password": "pw-123456789"})),
        )
        .await;
    assert_eq!(v["error"], "InvalidRequest", "a taken handle: {v}");
    // A duplicate app-password name.
    for expect in [None, Some("InvalidRequest")] {
        let (_, v) = h
            .call(
                "POST",
                "/xrpc/com.atproto.server.createAppPassword",
                Some(&access),
                Some(json!({"name": "one"})),
            )
            .await;
        assert_eq!(v["error"].as_str(), expect, "{v}");
    }
    // getBlocks serves only blocks a tree references: the empty genesis
    // node is gone from the tree after the first record.
    h.call(
        "POST",
        "/xrpc/com.atproto.repo.createRecord",
        Some(&access),
        Some(json!({"repo": did, "collection": "app.bsky.feed.post", "record": post_record("x")})),
    )
    .await;
    let empty = "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifvx5xytvske7mr3hpm";
    let (st, v) = h
        .call(
            "GET",
            &format!("/xrpc/com.atproto.sync.getBlocks?did={did}&cids={empty}"),
            None,
            None,
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("InvalidRequest")),
        "{v}"
    );
    // listRepos: a cursor on every page with rows.
    let (_, v) = h
        .call("GET", "/xrpc/com.atproto.sync.listRepos", None, None)
        .await;
    assert!(v["cursor"].is_string(), "{v}");
    // No default relay for a local hostname.
    let plan = crate::setup::seed_plan(&h.state.snapshot(), Some("localhost"));
    assert!(plan.services.crawlers.is_empty());
}
