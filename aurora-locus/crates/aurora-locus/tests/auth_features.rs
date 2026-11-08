//! `PDS_SERVICE_DID` through the real binary: recorded at setup, served as
//! the service identity, and a changed value refused on a later boot. A
//! mock PLC directory runs in the test.

use std::collections::HashMap;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::{Path as UrlPath, State};
use axum::routing::get;
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_aurora-locus");

type Ops = Arc<Mutex<HashMap<String, Value>>>;

/// A PLC directory that keeps the last op per DID.
async fn plc() -> String {
    async fn get_any(
        State(ops): State<Ops>,
        UrlPath(path): UrlPath<String>,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        let did = path
            .strip_suffix("/data")
            .or_else(|| path.strip_suffix("/log/last"))
            .unwrap_or(&path)
            .to_owned();
        match ops.lock().unwrap().get(&did).cloned() {
            Some(op) => axum::Json(op).into_response(),
            None => axum::http::StatusCode::NOT_FOUND.into_response(),
        }
    }
    async fn post_op(
        State(ops): State<Ops>,
        UrlPath(did): UrlPath<String>,
        axum::Json(op): axum::Json<Value>,
    ) {
        ops.lock().unwrap().insert(did, op);
    }
    let ops: Ops = Arc::default();
    let app = Router::new()
        .route("/{*path}", get(get_any).post(post_op))
        .with_state(ops);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://localhost:{}", l.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(l, app).await });
    url
}

/// A port for a child process to listen on. A port from `bind(0)` comes from
/// the ephemeral range, which outgoing connections draw from too, so one
/// could be taken between here and the child's bind ("Address already in
/// use"). These come from below that range (Linux's starts at 32768), a
/// different one on each call, skipping any in use.
fn free_port() -> u16 {
    use std::sync::atomic::{AtomicU32, Ordering};
    const FIRST: u32 = 20_000;
    const SPAN: u32 = 12_000;
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let start = std::process::id().wrapping_mul(7919);
    for _ in 0..SPAN {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let port = u16::try_from(FIRST + start.wrapping_add(n) % SPAN).unwrap();
        if std::net::TcpListener::bind(("0.0.0.0", port)).is_ok() {
            return port;
        }
    }
    panic!("no free port in {FIRST}..{}", FIRST + SPAN);
}

struct Node {
    child: Child,
    url: String,
    log: std::path::PathBuf,
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(self.child.id().to_string())
            .status();
        for _ in 0..200 {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts a node (headless setup on first boot) and waits until it serves.
async fn start(plc: &str, dir: &Path, service_did: Option<&str>, log: &str) -> Node {
    let port = free_port();
    let log = dir.with_extension(log);
    let file = std::fs::File::create(&log).unwrap();
    let mut c = Command::new(BIN);
    c.env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", std::env::var("HOME").unwrap_or_default())
        .env("LOCUS_DATA_DIR", dir)
        .env("PDS_HOSTNAME", "localhost")
        .env("PDS_DEV_MODE", "true")
        .env("LOCUS_PORT", port.to_string())
        .env("LOCUS_SETUP_AUTO", "true")
        .env("LOCUS_SETUP_OPERATOR_USERNAME", "op")
        .env("LOCUS_SETUP_OPERATOR_PASSWORD", "op-password-123")
        .env("LOCUS_SETUP_KEYS_BACKUP_ACKNOWLEDGED", "true")
        .env("PDS_SERVICE_HANDLE_DOMAINS", ".test")
        .env("PDS_DID_PLC_URL", plc)
        .env("PDS_INVITE_REQUIRED", "false")
        .env("LOCUS_LOG_FORMAT", "json");
    if let Some(d) = service_did {
        c.env("PDS_SERVICE_DID", d);
    }
    let mut child = c
        .arg("serve")
        .stdout(Stdio::from(file.try_clone().unwrap()))
        .stderr(Stdio::from(file))
        .spawn()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}");
    for _ in 0..240 {
        if describe(&url).await.is_some() {
            return Node { child, url, log };
        }
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!(
        "the node didn't come up: {}",
        std::fs::read_to_string(&log).unwrap_or_default()
    );
}

async fn describe(url: &str) -> Option<Value> {
    let r = reqwest::Client::new()
        .get(format!("{url}/xrpc/com.atproto.server.describeServer"))
        .send()
        .await
        .ok()?;
    if !r.status().is_success() {
        return None;
    }
    r.json().await.ok()
}

/// The `aud` of a legacy access token from a new account.
async fn access_aud(url: &str, handle: &str) -> Value {
    let v: Value = reqwest::Client::new()
        .post(format!("{url}/xrpc/com.atproto.server.createAccount"))
        .json(&json!({"handle": handle, "email": format!("{handle}@example.com"), "password": "pw-123456789"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let jwt = v["accessJwt"].as_str().unwrap_or_else(|| panic!("{v}"));
    use base64::Engine;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(jwt.split('.').nth(1).unwrap())
        .unwrap();
    serde_json::from_slice::<Value>(&payload).unwrap()["aud"].clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn pds_service_did_is_fixed_at_setup_and_a_change_is_refused() {
    const EXPLICIT: &str = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";
    let root = tempfile::tempdir().unwrap();
    let plc = plc().await;
    let dir = root.path().join("node");
    std::fs::create_dir_all(&dir).unwrap();

    // First boot, set up headless with PDS_SERVICE_DID: it's the service DID.
    let node = start(&plc, &dir, Some(EXPLICIT), "first.log").await;
    assert_eq!(describe(&node.url).await.unwrap()["did"], EXPLICIT);
    assert_eq!(access_aud(&node.url, "adeline.test").await, EXPLICIT);
    // The host's did.json keeps its did:web.
    let doc: Value = reqwest::Client::new()
        .get(format!("{}/.well-known/did.json", node.url))
        .header("host", "localhost")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(doc["id"], "did:web:localhost", "{doc}");
    drop(node);

    // A later boot with another value: refused and warned about; the
    // recorded one stays.
    let node = start(
        &plc,
        &dir,
        Some("did:plc:anotheranotheranother00"),
        "second.log",
    )
    .await;
    assert_eq!(describe(&node.url).await.unwrap()["did"], EXPLICIT);
    assert_eq!(access_aud(&node.url, "bobbie.test").await, EXPLICIT);
    let log = std::fs::read_to_string(&node.log).unwrap();
    assert!(
        log.contains("PDS_SERVICE_DID differs from the service DID setup recorded"),
        "{log}"
    );
    drop(node);

    // Unset: still the recorded one (and still warned about).
    let node = start(&plc, &dir, None, "third.log").await;
    assert_eq!(describe(&node.url).await.unwrap()["did"], EXPLICIT);
    drop(node);

    // An install set up without one is did:web:<host>, and stays so when
    // one appears later.
    let other = root.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    let node = start(&plc, &other, None, "plain.log").await;
    assert_eq!(
        describe(&node.url).await.unwrap()["did"],
        "did:web:localhost"
    );
    drop(node);
    let node = start(&plc, &other, Some(EXPLICIT), "later.log").await;
    assert_eq!(
        describe(&node.url).await.unwrap()["did"],
        "did:web:localhost"
    );
    let log = std::fs::read_to_string(&node.log).unwrap();
    assert!(log.contains("PDS_SERVICE_DID differs"), "{log}");
}
