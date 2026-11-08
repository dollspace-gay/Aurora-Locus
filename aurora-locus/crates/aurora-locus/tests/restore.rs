//! `backup restore` end to end, with the real binary: a node takes a
//! backup through the CLI, then restores run into fresh data dirs. A mock
//! PLC directory and a mock relay (`getHostStatus`) run in the test.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::{Path as UrlPath, State};
use axum::routing::get;
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_aurora-locus");

// ------------------------------------------------------------ mock services

type Ops = Arc<Mutex<HashMap<String, Value>>>;

/// The mock PLC: stores ops, serves `/data` and the document.
async fn plc() -> (String, Ops) {
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
        let op = ops.lock().unwrap().get(&did).cloned();
        match op {
            Some(op) if path.ends_with("/data") || path.ends_with("/log/last") => axum::Json(op).into_response(),
            Some(op) => axum::Json(json!({
                "@context": ["https://www.w3.org/ns/did/v1"],
                "id": did,
                "alsoKnownAs": op["alsoKnownAs"],
                "verificationMethod": [{"id": format!("{did}#atproto"), "type": "Multikey", "controller": did,
                    "publicKeyMultibase": op["verificationMethods"]["atproto"].as_str().unwrap_or("").trim_start_matches("did:key:")}],
                "service": [{"id": "#atproto_pds", "type": "AtprotoPersonalDataServer",
                    "serviceEndpoint": op["services"]["atproto_pds"]["endpoint"]}],
            }))
            .into_response(),
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
        .with_state(Arc::clone(&ops));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://localhost:{}", l.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(l, app).await });
    (url, ops)
}

/// The mock relay: `getHostStatus` answers the seq set (negative: 500).
async fn relay() -> (String, Arc<AtomicI64>) {
    let seq = Arc::new(AtomicI64::new(-1));
    let s = Arc::clone(&seq);
    let app = Router::new()
        .route(
            "/xrpc/com.atproto.sync.getHostStatus",
            get(move || {
                let s = Arc::clone(&s);
                async move {
                    let v = s.load(Ordering::SeqCst);
                    if v < 0 {
                        Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
                    } else {
                        Ok(axum::Json(
                            json!({"hostname": "localhost", "seq": v, "status": "active"}),
                        ))
                    }
                }
            }),
        )
        .route(
            "/xrpc/com.atproto.sync.requestCrawl",
            axum::routing::post(|| async { "{}" }),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(l, app).await });
    (url, seq)
}

// ------------------------------------------------------------ the binary

struct Env {
    plc: String,
    relay: String,
    /// More variables (a Postgres install's database and master key).
    extra: Vec<(String, String)>,
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

fn command(env: &Env, dir: &Path, port: u16) -> Command {
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
        .env("PDS_ADMIN_PASSWORD", "admin-pw")
        .env("PDS_SERVICE_HANDLE_DOMAINS", ".test")
        .env("PDS_DID_PLC_URL", &env.plc)
        .env("PDS_INVITE_REQUIRED", "false")
        .env("PDS_CRAWLERS", &env.relay)
        .env("LOCUS_LOG_FORMAT", "json")
        .envs(env.extra.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    c
}

fn run(env: &Env, dir: &Path, args: &[&str]) -> Output {
    command(env, dir, free_port()).args(args).output().unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

struct Node {
    child: Child,
    url: String,
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn start(env: &Env, dir: &Path) -> Node {
    let port = free_port();
    let log = std::fs::File::create(dir.with_extension("log")).unwrap();
    let mut child = command(env, dir, port)
        .arg("serve")
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    for _ in 0..240 {
        if let Ok(r) = client
            .get(format!("{url}/xrpc/com.atproto.server.describeServer"))
            .send()
            .await
            && r.status().is_success()
        {
            return Node { child, url };
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!(
        "the node didn't come up: {}",
        std::fs::read_to_string(dir.with_extension("log")).unwrap_or_default()
    );
}

async fn xrpc(
    url: &str,
    method: &str,
    nsid: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (u16, Value) {
    let c = reqwest::Client::new();
    let mut r = if method == "GET" {
        c.get(format!("{url}/xrpc/{nsid}"))
    } else {
        c.post(format!("{url}/xrpc/{nsid}"))
    };
    if let Some(t) = token {
        r = r.bearer_auth(t);
    }
    if let Some(b) = body {
        r = r.json(&b);
    }
    let res = r.send().await.unwrap();
    let st = res.status().as_u16();
    (st, res.json().await.unwrap_or(Value::Null))
}

async fn db(dir: &Path) -> sqlx::SqlitePool {
    sqlx::SqlitePool::connect(&format!(
        "sqlite://{}?mode=ro",
        dir.join("locus.sqlite").display()
    ))
    .await
    .unwrap()
}

/// A fresh data dir holding the original node's master key.
fn fresh_dir(root: &Path, name: &str, key_from: &Path) -> PathBuf {
    let d = root.join(name);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::copy(key_from.join("master.key"), d.join("master.key")).unwrap();
    d
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let p = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &p);
        } else {
            std::fs::copy(e.path(), p).unwrap();
        }
    }
}

fn next_seq(report: &str) -> i64 {
    let i =
        report.find("The next event's seq is ").expect(report) + "The next event's seq is ".len();
    report[i..]
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

// ------------------------------------------------------------ the test

#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread")]
async fn backups_restore_into_empty_installs_only_and_announce_themselves() {
    let root = tempfile::tempdir().unwrap();
    let (plc_url, ops) = plc().await;
    let (relay_url, relay_seq) = relay().await;
    let env = Env {
        plc: plc_url,
        relay: relay_url,
        extra: Vec::new(),
    };

    // The original install: two accounts, posts and a blob.
    let a = root.path().join("a");
    std::fs::create_dir_all(&a).unwrap();
    let node = start(&env, &a).await;
    let mut dids = Vec::new();
    let mut alice_token = String::new();
    for h in ["alice.test", "bobby.test"] {
        let (st, v) = xrpc(
            &node.url,
            "POST",
            "com.atproto.server.createAccount",
            None,
            Some(json!({"handle": h, "email": format!("{h}@example.com"), "password": "pw-123456789"})),
        )
        .await;
        assert_eq!(st, 200, "{v}");
        dids.push(v["did"].as_str().unwrap().to_owned());
        if alice_token.is_empty() {
            alice_token = v["accessJwt"].as_str().unwrap().to_owned();
        }
    }
    let alice = dids[0].clone();
    let res = reqwest::Client::new()
        .post(format!("{}/xrpc/com.atproto.repo.uploadBlob", node.url))
        .bearer_auth(&alice_token)
        .header("content-type", "image/png")
        .body(b"backed up bytes".to_vec())
        .send()
        .await
        .unwrap();
    let blob: Value = res.json().await.unwrap();
    let blob_cid = blob["blob"]["ref"]["$link"].as_str().unwrap().to_owned();
    let (st, post) = xrpc(
        &node.url,
        "POST",
        "com.atproto.repo.createRecord",
        Some(&alice_token),
        Some(json!({"repo": alice, "collection": "app.bsky.feed.post", "record": {
            "$type": "app.bsky.feed.post", "text": "kept", "createdAt": "2026-11-06T00:00:00.000Z",
            "embed": {"$type": "app.bsky.embed.images", "images": [{"alt": "", "image": blob["blob"]}]}}})),
    )
    .await;
    assert_eq!(st, 200, "{post}");
    let post_uri = post["uri"].as_str().unwrap().to_owned();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let passphrase = root.path().join("pass");
    std::fs::write(&passphrase, "backup passphrase 9").unwrap();
    let out = run(
        &env,
        &a,
        &[
            "backup",
            "create",
            "--passphrase-file",
            passphrase.to_str().unwrap(),
            "--wait",
        ],
    );
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    let backup = PathBuf::from(
        said.split("into ")
            .nth(1)
            .and_then(|s| s.split(".\n").next())
            .expect(&said),
    );
    let snapshot_rev: String = sqlx::query_scalar("SELECT rev FROM repo_root WHERE did = ?1")
        .bind(&alice)
        .fetch_one(&db(&a).await)
        .await
        .unwrap();
    drop(node);
    let restore = |dir: &Path, extra: &[&str]| {
        let mut args = vec![
            "backup",
            "restore",
            backup.to_str().unwrap(),
            "--backend",
            "disk",
        ];
        args.extend_from_slice(extra);
        run(&env, dir, &args)
    };

    // Refused over an existing database, which stays byte for byte.
    let b = fresh_dir(root.path(), "exists", &a);
    std::fs::write(b.join("locus.sqlite"), b"not yours").unwrap();
    let out = restore(&b, &[]);
    assert!(!out.status.success());
    assert!(text(&out).contains("a database exists"), "{}", text(&out));
    assert_eq!(std::fs::read(b.join("locus.sqlite")).unwrap(), b"not yours");

    // Refused with another master key; nothing written.
    let b = root.path().join("otherkey");
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(b.join("master.key"), "11".repeat(32)).unwrap();
    let out = restore(&b, &[]);
    assert!(!out.status.success());
    assert!(text(&out).contains("master key"), "{}", text(&out));
    assert!(!b.join("locus.sqlite").exists());
    assert!(!b.join("locus.restoring").exists());

    // A failed backup: refused, then restored with --accept-failed.
    let failed = root.path().join("failed-backup");
    copy_dir(&backup, &failed);
    let mut m: Value =
        serde_json::from_slice(&std::fs::read(failed.join("manifest.json")).unwrap()).unwrap();
    m["status"] = json!("failed");
    m["error"] = json!("lease lost");
    std::fs::write(failed.join("manifest.json"), m.to_string()).unwrap();
    let b = fresh_dir(root.path(), "failed", &a);
    let args = |extra: &[&'static str]| {
        let mut v = vec![
            "backup",
            "restore",
            failed.to_str().unwrap(),
            "--backend",
            "disk",
        ];
        v.extend_from_slice(extra);
        v
    };
    let out = run(&env, &b, &args(&[]));
    assert!(!out.status.success());
    assert!(text(&out).contains("--accept-failed"), "{}", text(&out));
    assert!(!b.join("locus.sqlite").exists());
    let out = run(&env, &b, &args(&["--accept-failed"]));
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("Restoring a failed backup"));

    // --emit-sync without a floor, and no relay answering: refused.
    let b = fresh_dir(root.path(), "nofloor", &a);
    let out = restore(&b, &["--emit-sync"]);
    assert!(!out.status.success());
    assert!(
        text(&out).contains("--emit-sync needs a seq floor"),
        "{}",
        text(&out)
    );
    assert!(!b.join("locus.sqlite").exists());

    // No relay answering, no floor: the snapshot's seq, with the warning.
    let b = fresh_dir(root.path(), "warned", &a);
    let out = restore(&b, &[]);
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(said.contains("FutureCursor"), "{said}");
    assert!(said.contains("didn't answer getHostStatus"), "{said}");
    let snapshot_last: i64 = sqlx::query_scalar("SELECT last_seq FROM seq_state")
        .fetch_one(&db(&b).await)
        .await
        .unwrap();
    assert_eq!(next_seq(&said), snapshot_last + 1);

    // The relay saw seq 50: the floor is 50 plus the margin. Alice's live
    // document then names another key: listed, and no fresh commit.
    relay_seq.store(50, Ordering::SeqCst);
    let real_op = ops.lock().unwrap().get(&alice).cloned().unwrap();
    let mut moved = real_op.clone();
    moved["verificationMethods"]["atproto"] =
        json!("did:key:zQ3shZc2QzApp2oymGvQbzP8eKheVshBHbU4ZYjeXqwSKEn6N");
    ops.lock().unwrap().insert(alice.clone(), moved);
    let b = fresh_dir(root.path(), "mismatch", &a);
    let out = restore(&b, &["--emit-sync"]);
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert_eq!(next_seq(&said), 50 + 1_000_000 + 1, "{said}");
    assert!(
        said.contains(&format!("{alice}: its DID document lists")),
        "{said}"
    );
    assert!(
        said.contains("Wrote a fresh commit and #sync for 1 active accounts"),
        "{said}"
    );
    let pool = db(&b).await;
    let rev: String = sqlx::query_scalar("SELECT rev FROM repo_root WHERE did = ?1")
        .bind(&alice)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rev, snapshot_rev, "the listed account gets no fresh commit");
    ops.lock().unwrap().insert(alice.clone(), real_op);

    // The real restore: every object, both accounts announced, a fresh
    // commit and #sync each (no #commit), then a node serving it all.
    let b = fresh_dir(root.path(), "restored", &a);
    let out = restore(&b, &["--emit-sync"]);
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(said.contains("Re-announced 2 accounts"), "{said}");
    assert!(
        said.contains("Wrote a fresh commit and #sync for 2 active accounts"),
        "{said}"
    );
    let pool = db(&b).await;
    let keys: Vec<String> = sqlx::query_scalar("SELECT storage_key FROM blob")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(!keys.is_empty());
    for k in &keys {
        assert!(b.join(k).exists(), "{k} restored");
    }
    let kinds: Vec<(String, String)> = sqlx::query_as("SELECT did, kind FROM outbox ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    for did in &dids {
        let mine: Vec<&str> = kinds
            .iter()
            .filter(|(d, _)| d == did)
            .map(|(_, k)| k.as_str())
            .collect();
        assert_eq!(
            mine,
            ["identity", "account", "sync"],
            "{did}: announced before its #sync"
        );
    }
    let rev: String = sqlx::query_scalar("SELECT rev FROM repo_root WHERE did = ?1")
        .bind(&alice)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(rev > snapshot_rev, "a fresh commit with a later rev");
    pool.close().await;

    let node = start(&env, &b).await;
    let rkey = post_uri.rsplit('/').next().unwrap();
    let (st, v) = xrpc(
        &node.url,
        "GET",
        &format!(
            "com.atproto.repo.getRecord?repo={alice}&collection=app.bsky.feed.post&rkey={rkey}"
        ),
        None,
        None,
    )
    .await;
    assert_eq!(
        (st, v["value"]["text"].as_str()),
        (200, Some("kept")),
        "{v}"
    );
    let bytes = reqwest::get(format!(
        "{}/xrpc/com.atproto.sync.getBlob?did={alice}&cid={blob_cid}",
        node.url
    ))
    .await
    .unwrap()
    .bytes()
    .await
    .unwrap();
    assert_eq!(&bytes[..], b"backed up bytes");
    let (st, _) = xrpc(
        &node.url,
        "POST",
        "com.atproto.server.createSession",
        None,
        Some(json!({"identifier": "alice.test", "password": "pw-123456789"})),
    )
    .await;
    assert_eq!(st, 200);
    // The events go out above the floor.
    let mut first = 0;
    for _ in 0..40 {
        let s: Option<i64> = sqlx::query_scalar("SELECT MIN(seq) FROM seq_event WHERE seq > ?1")
            .bind(1_000_050)
            .fetch_one(&db(&b).await)
            .await
            .unwrap();
        if let Some(s) = s {
            first = s;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(
        first,
        50 + 1_000_000 + 1,
        "the first event after the restore is just above the floor"
    );
}

/// `backup post-restore` after a Postgres point-in-time recovery: run on
/// the recovered database (here, a stopped install's) before any node.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread")]
async fn post_restore_checks_objects_ends_migrations_and_raises_the_floor() {
    let Ok(base) = std::env::var("LOCUS_TEST_POSTGRES_URL") else {
        return;
    };
    let admin = sqlx::PgPool::connect(&base).await.unwrap();
    let name = format!("locus_test_postrestore_{}", std::process::id());
    let empty = format!("{name}_empty");
    for n in [&name, &empty] {
        sqlx::query(&format!("DROP DATABASE IF EXISTS {n}"))
            .execute(&admin)
            .await
            .unwrap();
        sqlx::query(&format!("CREATE DATABASE {n}"))
            .execute(&admin)
            .await
            .unwrap();
    }
    let url_of = |n: &str| {
        let (head, _) = base.rsplit_once('/').unwrap();
        format!("{head}/{n}")
    };
    let root = tempfile::tempdir().unwrap();
    let (plc_url, _ops) = plc().await;
    let (relay_url, relay_seq) = relay().await;
    let key = "22".repeat(32);
    let env_for = |db: &str| Env {
        plc: plc_url.clone(),
        relay: relay_url.clone(),
        extra: vec![
            ("LOCUS_DATABASE_URL".into(), db.to_owned()),
            ("LOCUS_MASTER_KEY_SOURCE".into(), "env".into()),
            ("LOCUS_MASTER_KEY".into(), key.clone()),
        ],
    };
    let env = env_for(&url_of(&name));

    // An empty install is refused (that's `backup restore`).
    let e = root.path().join("empty");
    std::fs::create_dir_all(&e).unwrap();
    let out = run(&env_for(&url_of(&empty)), &e, &["backup", "post-restore"]);
    assert!(!out.status.success());
    assert!(text(&out).contains("empty"), "{}", text(&out));

    // An install with three blobs, then stopped.
    let a = root.path().join("a");
    std::fs::create_dir_all(&a).unwrap();
    let node = start(&env, &a).await;
    let (st, v) = xrpc(
        &node.url,
        "POST",
        "com.atproto.server.createAccount",
        None,
        Some(json!({"handle": "pat.test", "email": "pat@example.com", "password": "pw-123456789"})),
    )
    .await;
    assert_eq!(st, 200, "{v}");
    let token = v["accessJwt"].as_str().unwrap().to_owned();
    for i in 0..3u8 {
        let r = reqwest::Client::new()
            .post(format!("{}/xrpc/com.atproto.repo.uploadBlob", node.url))
            .bearer_auth(&token)
            .header("content-type", "image/png")
            .body(vec![b'x', i])
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success());
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    drop(node);
    let pool = sqlx::PgPool::connect(&url_of(&name)).await.unwrap();
    let keys: Vec<String> = sqlx::query_scalar("SELECT storage_key FROM blob ORDER BY storage_key")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(keys.len(), 3);
    let last_seq = || async {
        sqlx::query_scalar::<_, i64>("SELECT last_seq FROM seq_state")
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    let before = last_seq().await;

    // --emit-sync with no floor and no relay answering: refused, nothing changed.
    let out = run(&env, &a, &["backup", "post-restore", "--emit-sync"]);
    assert!(!out.status.success());
    assert!(
        text(&out).contains("--emit-sync needs a seq floor"),
        "{}",
        text(&out)
    );
    assert_eq!(last_seq().await, before);

    // Two of three objects gone: refused unless accepted.
    std::fs::remove_file(a.join(&keys[0])).unwrap();
    std::fs::remove_file(a.join(&keys[1])).unwrap();
    let out = run(&env, &a, &["backup", "post-restore"]);
    assert!(!out.status.success());
    assert!(
        text(&out).contains("2 of 3 objects are missing"),
        "{}",
        text(&out)
    );
    assert_eq!(last_seq().await, before);

    // --backend naming another disk root: what the restored backend still
    // holds is copied across, the epoch moves; a migration that was
    // running ends; the floor comes from the relay.
    sqlx::query(
        "INSERT INTO blob_migration (id, target, phase, started_at, started_by)          VALUES ('m1', 'disk', 'copying', 0, 'test')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE blob_backend SET migration_job = 'm1' WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap();
    let epoch: i64 = sqlx::query_scalar("SELECT epoch FROM blob_backend WHERE id = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    relay_seq.store(7, Ordering::SeqCst);
    let other = root.path().join("other-root");
    std::fs::create_dir_all(&other).unwrap();
    let out = run(
        &env,
        &a,
        &[
            "backup",
            "post-restore",
            "--backend",
            "disk",
            "--disk-root",
            other.to_str().unwrap(),
            "--emit-sync",
        ],
    );
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains(&format!("Copied {} from the restored backend", keys[2])),
        "{said}"
    );
    assert!(
        said.contains(&format!("Unrecoverable: {}", keys[0])),
        "{said}"
    );
    assert!(
        said.contains("Checked 3 objects: 3 were missing, 2 couldn't be recovered"),
        "{said}"
    );
    assert!(
        said.contains("A blob migration that was running was ended"),
        "{said}"
    );
    assert!(other.join(&keys[2]).exists());
    assert_eq!(next_seq(&said), 7 + 1_000_000 + 1);
    assert_eq!(last_seq().await, 7 + 1_000_000);
    let (active, new_epoch, root_now, job): (String, i64, Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT active, epoch, disk_root, migration_job FROM blob_backend WHERE id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(active, "disk");
    assert_eq!(new_epoch, epoch + 1, "a new backend is a new epoch");
    assert_eq!(root_now.as_deref(), other.to_str());
    assert!(job.is_none());
    let phase: String = sqlx::query_scalar("SELECT phase FROM blob_migration WHERE id = 'm1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(phase, "cancelled");
    let syncs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbox WHERE kind = 'sync'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(syncs, 1, "a fresh commit and #sync for the active account");
    pool.close().await;
    for n in [&name, &empty] {
        let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {n} WITH (FORCE)"))
            .execute(&admin)
            .await;
    }
}
