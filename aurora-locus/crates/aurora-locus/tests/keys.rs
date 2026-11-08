//! The master key end to end, with the real binary: `keys rewrap` killed,
//! failed and refused, nodes refusing a key that isn't the install's, and
//! (on Postgres) nodes racing to make the first secret with different keys.
//! A mock PLC directory runs in the test.

use std::collections::HashMap;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::{Path as UrlPath, State};
use axum::routing::get;
use locus_core::keys::{MasterKey, SourceKind};
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_aurora-locus");

// ------------------------------------------------------------ mock PLC

type Ops = Arc<Mutex<HashMap<String, Value>>>;

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
        let op = ops.lock().unwrap().get(&did).cloned();
        match op {
            Some(op) if path.ends_with("/data") || path.ends_with("/log/last") => {
                axum::Json(op).into_response()
            }
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
        .with_state(ops);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://localhost:{}", l.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(l, app).await });
    url
}

// ------------------------------------------------------------ the binary

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

/// The binary with a clean env; `prefix` runs it through `sh -c` (for
/// `ulimit` and `trap`).
fn command(plc: &str, dir: &Path, port: u16, extra: &[(&str, &str)], prefix: &str) -> Command {
    let mut c = if prefix.is_empty() {
        Command::new(BIN)
    } else {
        let mut c = Command::new("sh");
        c.arg("-c")
            .arg(format!("{prefix}; exec \"$0\" \"$@\""))
            .arg(BIN);
        c
    };
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
        .env("PDS_DID_PLC_URL", plc)
        .env("PDS_INVITE_REQUIRED", "false")
        .env("LOCUS_LOG_FORMAT", "json")
        .envs(extra.iter().copied());
    c
}

fn run(plc: &str, dir: &Path, extra: &[(&str, &str)], args: &[&str]) -> Output {
    command(plc, dir, free_port(), extra, "")
        .args(args)
        .output()
        .unwrap()
}

fn args(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
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
    /// Stops the node the way an operator does (SIGTERM), so it releases
    /// its slot and the database is left clean.
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

fn spawn(plc: &str, dir: &Path, extra: &[(&str, &str)], log: &Path) -> (Child, String) {
    let port = free_port();
    let log = std::fs::File::create(log).unwrap();
    let child = command(plc, dir, port, extra, "")
        .arg("serve")
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .unwrap();
    (child, format!("http://127.0.0.1:{port}"))
}

async fn up(url: &str) -> bool {
    reqwest::Client::new()
        .get(format!("{url}/xrpc/com.atproto.server.describeServer"))
        .send()
        .await
        .is_ok_and(|r| r.status().is_success())
}

/// Starts a node and waits until it serves.
async fn start(plc: &str, dir: &Path, extra: &[(&str, &str)]) -> Node {
    let log = dir.with_extension("log");
    let (mut child, url) = spawn(plc, dir, extra, &log);
    for _ in 0..240 {
        if up(&url).await {
            return Node { child, url };
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

/// Starts a node that must refuse to start; returns its log.
async fn refused(plc: &str, dir: &Path, extra: &[(&str, &str)]) -> String {
    let log = dir.with_extension("refused.log");
    let (mut child, url) = spawn(plc, dir, extra, &log);
    for _ in 0..240 {
        if let Ok(Some(st)) = child.try_wait() {
            assert!(!st.success());
            return std::fs::read_to_string(&log).unwrap();
        }
        if up(&url).await {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the node started");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("the node neither started nor exited");
}

async fn xrpc(url: &str, nsid: &str, token: Option<&str>, body: Value) -> (u16, Value) {
    let mut r = reqwest::Client::new()
        .post(format!("{url}/xrpc/{nsid}"))
        .json(&body);
    if let Some(t) = token {
        r = r.bearer_auth(t);
    }
    let res = r.send().await.unwrap();
    let st = res.status().as_u16();
    (st, res.json().await.unwrap_or(Value::Null))
}

/// Creates an account; returns (did, access token).
async fn account(url: &str, handle: &str) -> (String, String) {
    let (st, v) = xrpc(
        url,
        "com.atproto.server.createAccount",
        None,
        json!({"handle": handle, "email": format!("{handle}@example.com"), "password": "pw-123456789"}),
    )
    .await;
    assert_eq!(st, 200, "{v}");
    (
        v["did"].as_str().unwrap().to_owned(),
        v["accessJwt"].as_str().unwrap().to_owned(),
    )
}

/// A post, signed with the account's stored signing key.
async fn post(url: &str, did: &str, token: &str, text: &str) {
    let (st, v) = xrpc(
        url,
        "com.atproto.repo.createRecord",
        Some(token),
        json!({"repo": did, "collection": "app.bsky.feed.post", "record": {
            "$type": "app.bsky.feed.post", "text": text, "createdAt": "2026-11-06T00:00:00.000Z"}}),
    )
    .await;
    assert_eq!(st, 200, "{v}");
}

// ------------------------------------------------------------ the key store

/// Everything the master key guards: the key check, and every sealed
/// secret and signing key, as stored.
#[derive(Debug, PartialEq, Eq)]
struct Sealed {
    check: Option<(String, Vec<u8>)>,
    secrets: Vec<(String, Vec<u8>)>,
    signing: Vec<(String, Vec<u8>)>,
}

impl Sealed {
    /// Whether every secret opens under `key`.
    fn opens_under(&self, key: &MasterKey) -> bool {
        self.secrets
            .iter()
            .all(|(n, s)| key.open(format!("secret:{n}").as_bytes(), s).is_ok())
            && self
                .signing
                .iter()
                .all(|(d, s)| key.open(format!("signing_key:{d}").as_bytes(), s).is_ok())
    }

    /// Whether `key` is the one the check row names.
    fn checks(&self, key: &MasterKey) -> bool {
        self.check
            .as_ref()
            .is_some_and(|(id, mac)| key.verifies(id, mac))
    }
}

async fn sqlite(dir: &Path) -> sqlx::SqlitePool {
    sqlx::SqlitePool::connect(&format!("sqlite://{}", dir.join("locus.sqlite").display()))
        .await
        .unwrap()
}

async fn sealed_sqlite(dir: &Path) -> Sealed {
    let p = sqlite(dir).await;
    let s = Sealed {
        check: sqlx::query_as("SELECT key_id, mac FROM master_key_check")
            .fetch_optional(&p)
            .await
            .unwrap(),
        secrets: sqlx::query_as("SELECT name, sealed FROM secret ORDER BY name")
            .fetch_all(&p)
            .await
            .unwrap(),
        signing: sqlx::query_as("SELECT did, sealed FROM signing_key ORDER BY did")
            .fetch_all(&p)
            .await
            .unwrap(),
    };
    p.close().await;
    s
}

async fn sealed_pg(url: &str) -> Sealed {
    let p = sqlx::PgPool::connect(url).await.unwrap();
    let s = Sealed {
        check: sqlx::query_as("SELECT key_id, mac FROM master_key_check")
            .fetch_optional(&p)
            .await
            .unwrap(),
        secrets: sqlx::query_as("SELECT name, sealed FROM secret ORDER BY name")
            .fetch_all(&p)
            .await
            .unwrap(),
        signing: sqlx::query_as("SELECT did, sealed FROM signing_key ORDER BY did")
            .fetch_all(&p)
            .await
            .unwrap(),
    };
    p.close().await;
    s
}

fn file_key(path: &Path) -> MasterKey {
    MasterKey::from_hex(&std::fs::read_to_string(path).unwrap(), SourceKind::File).unwrap()
}

// ------------------------------------------------------------ the tests

#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread")]
async fn a_rewrap_changes_nothing_until_it_commits_and_then_retires_the_old_key() {
    let root = tempfile::tempdir().unwrap();
    let plc = plc().await;
    let a = root.path().join("a");
    std::fs::create_dir_all(&a).unwrap();
    let old_file = a.join("master.key");

    // An install with an account (its signing key is a secret too).
    let node = start(&plc, &a, &[]).await;
    let (alice, token) = account(&node.url, "alice.test").await;
    post(&node.url, &alice, &token, "before").await;
    drop(node);
    let old = file_key(&old_file);
    let old_bytes = std::fs::read(&old_file).unwrap();

    // Enough secrets that the rewrap's writes are many pages, so a write
    // limit stops it partway through.
    let p = sqlite(&a).await;
    for i in 0..3000 {
        let name = format!("bulk-{i:05}");
        let sealed = old
            .seal(format!("secret:{name}").as_bytes(), &[7u8; 300])
            .unwrap();
        sqlx::query("INSERT INTO secret (name, sealed, updated_at) VALUES (?1, ?2, 0)")
            .bind(&name)
            .bind(sealed)
            .execute(&p)
            .await
            .unwrap();
    }
    p.close().await;
    let before = sealed_sqlite(&a).await;
    assert!(before.checks(&old) && before.opens_under(&old));
    let unchanged = |why: &str, now: &Sealed| {
        assert_eq!(&before, now, "{why}: the key store changed");
        assert_eq!(
            std::fs::read(&old_file).unwrap(),
            old_bytes,
            "{why}: the old key file changed"
        );
    };
    let rewrap = |path: &Path| {
        vec![
            "keys".to_owned(),
            "rewrap".into(),
            "--to".into(),
            "file".into(),
            "--path".into(),
            path.display().to_string(),
        ]
    };

    // Refused while a node runs, before writing the new key file.
    let node = start(&plc, &a, &[]).await;
    let busy = root.path().join("busy.key");
    let out = run(&plc, &a, &[], &args(&rewrap(&busy)));
    assert!(!out.status.success());
    assert!(text(&out).contains("a node is running"), "{}", text(&out));
    assert!(!busy.exists());
    drop(node);
    let before = sealed_sqlite(&a).await;

    // The current key file as --path: refused.
    let out = run(&plc, &a, &[], &args(&rewrap(&old_file)));
    assert!(!out.status.success());
    assert!(text(&out).contains("new path"), "{}", text(&out));
    unchanged("--path the current file", &sealed_sqlite(&a).await);

    // A full disk partway through (file writes past 64 KiB fail): the
    // rewrap fails and nothing changed.
    let limit = "ulimit -f 128";
    let full = root.path().join("full.key");
    let out = command(
        &plc,
        &a,
        free_port(),
        &[],
        &format!("trap '' XFSZ; {limit}"),
    )
    .args(args(&rewrap(&full)))
    .output()
    .unwrap();
    assert!(!out.status.success());
    assert!(out.status.code().is_some(), "it failed, not killed");
    assert!(text(&out).contains("disk"), "{}", text(&out));
    unchanged("a full disk", &sealed_sqlite(&a).await);

    // Killed partway through (SIGXFSZ at the same point): nothing changed.
    let killed = root.path().join("killed.key");
    let out = command(&plc, &a, free_port(), &[], limit)
        .args(args(&rewrap(&killed)))
        .output()
        .unwrap();
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            out.status.signal(),
            Some(25),
            "killed by SIGXFSZ: {}",
            text(&out)
        );
    }
    unchanged("killed", &sealed_sqlite(&a).await);

    // A secret that won't decrypt, streamed last: the rewrap fails there,
    // after re-sealing everything before it, and nothing changed.
    let p = sqlite(&a).await;
    let mut junk = vec![1u8];
    junk.extend_from_slice(&[0u8; 60]);
    sqlx::query("INSERT INTO signing_key (did, key_type, sealed, created_at) VALUES ('did:plc:zzzzzzzz', 'k256', ?1, 0)")
        .bind(&junk)
        .execute(&p)
        .await
        .unwrap();
    p.close().await;
    let with_junk = sealed_sqlite(&a).await;
    let corrupt = root.path().join("corrupt.key");
    let out = run(&plc, &a, &[], &args(&rewrap(&corrupt)));
    assert!(!out.status.success());
    assert!(text(&out).contains("failed to decrypt"), "{}", text(&out));
    assert_eq!(with_junk, sealed_sqlite(&a).await, "a failed decrypt");
    let p = sqlite(&a).await;
    sqlx::query("DELETE FROM signing_key WHERE did = 'did:plc:zzzzzzzz'")
        .execute(&p)
        .await
        .unwrap();
    p.close().await;
    unchanged("a failed decrypt", &sealed_sqlite(&a).await);

    // After all that, the node boots on the old key and signs as before.
    let node = start(&plc, &a, &[]).await;
    post(&node.url, &alice, &token, "still the old key").await;
    drop(node);

    // A key that isn't the install's is refused before migrations run: a
    // pending migration is still pending afterwards (on a copy).
    let c = root.path().join("copy");
    std::fs::create_dir_all(&c).unwrap();
    for f in ["locus.sqlite", "locus.sqlite-wal", "master.key"] {
        if a.join(f).exists() {
            std::fs::copy(a.join(f), c.join(f)).unwrap();
        }
    }
    let p = sqlite(&c).await;
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM master_key_check")
        .fetch_one(&p)
        .await
        .unwrap();
    p.close().await;
    assert_eq!(rows, 1, "the copy has the key check");
    let p = sqlite(&c).await;
    let last: i64 = sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations")
        .fetch_one(&p)
        .await
        .unwrap();
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version = ?1")
        .bind(last)
        .execute(&p)
        .await
        .unwrap();
    p.close().await;
    let other = "33".repeat(32);
    let env_key = [
        ("LOCUS_MASTER_KEY_SOURCE", "env"),
        ("LOCUS_MASTER_KEY", other.as_str()),
    ];
    let log = refused(&plc, &c, &env_key).await;
    assert!(log.contains("not the install's key"), "{log}");
    assert!(log.contains("from env"), "says which source: {log}");
    let p = sqlite(&c).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE version = ?1")
        .bind(last)
        .fetch_one(&p)
        .await
        .unwrap();
    p.close().await;
    assert_eq!(n, 0, "a migration ran under the wrong key");

    // `migrate storage` refuses this install's generated key file.
    let out = run(
        &plc,
        &a,
        &[],
        &[
            "migrate",
            "storage",
            "--to",
            "postgres",
            "--url",
            "postgres://127.0.0.1:9/none",
        ],
    );
    assert!(!out.status.success());
    assert!(
        text(&out).contains("LOCUS_MASTER_KEY_SHARED_FILE"),
        "{}",
        text(&out)
    );

    // The rewrap: every secret moves to the new key, the old file stays.
    let new_file = root.path().join("keys").join("new.key");
    let out = run(&plc, &a, &[], &args(&rewrap(&new_file)));
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("keys backup"), "{}", text(&out));
    assert_eq!(std::fs::read(&old_file).unwrap(), old_bytes);
    let new = file_key(&new_file);
    let after = sealed_sqlite(&a).await;
    assert!(after.checks(&new) && !after.checks(&old));
    assert!(after.opens_under(&new));
    assert_eq!(after.secrets.len(), before.secrets.len());
    assert_eq!(after.signing.len(), before.signing.len());
    assert!(
        after
            .secrets
            .iter()
            .all(|(n, s)| old.open(format!("secret:{n}").as_bytes(), s).is_err())
    );

    // A node still on the old file refuses to start; on the new one it
    // serves, and signs with the account's re-sealed key.
    let log = refused(&plc, &a, &[]).await;
    assert!(log.contains("not the install's key"), "{log}");
    let new_path = new_file.display().to_string();
    let node = start(&plc, &a, &[("LOCUS_MASTER_KEY_FILE", new_path.as_str())]).await;
    post(&node.url, &alice, &token, "under the new key").await;
    drop(node);
}

/// On Postgres: no per-node key file, one key check however nodes race,
/// and a node with another key never writes.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread")]
async fn postgres_nodes_share_one_master_key() {
    let Ok(base) = std::env::var("LOCUS_TEST_POSTGRES_URL") else {
        return;
    };
    let admin = sqlx::PgPool::connect(&base).await.unwrap();
    let fresh = |n: &str| {
        let admin = admin.clone();
        let name = format!("locus_test_keys_{}_{n}", std::process::id());
        let (head, _) = base.rsplit_once('/').unwrap();
        let url = format!("{head}/{name}");
        async move {
            sqlx::query(&format!("DROP DATABASE IF EXISTS {name}"))
                .execute(&admin)
                .await
                .unwrap();
            sqlx::query(&format!("CREATE DATABASE {name}"))
                .execute(&admin)
                .await
                .unwrap();
            url
        }
    };
    let root = tempfile::tempdir().unwrap();
    let plc = plc().await;
    let dir = |n: &str| {
        let d = root.path().join(n);
        std::fs::create_dir_all(&d).unwrap();
        d
    };

    // The default key file, not declared shared: refused, no key generated
    // and nothing written; an existing file isn't used either.
    let url = fresh("file").await;
    let d = dir("file");
    let db = [("LOCUS_DATABASE_URL", url.as_str())];
    let log = refused(&plc, &d, &db).await;
    assert!(log.contains("LOCUS_MASTER_KEY_SHARED_FILE"), "{log}");
    assert!(!d.join("master.key").exists());
    std::fs::write(d.join("master.key"), "44".repeat(32)).unwrap();
    let log = refused(&plc, &d, &db).await;
    assert!(log.contains("LOCUS_MASTER_KEY_SHARED_FILE"), "{log}");
    let p = sqlx::PgPool::connect(&url).await.unwrap();
    let tables: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public'",
    )
    .fetch_one(&p)
    .await
    .unwrap();
    p.close().await;
    assert_eq!(tables, 0, "a refused node wrote to the database");

    // Two nodes setting up a new install at once, each with its own key:
    // one wins, the other exits, and every secret is under the winner's.
    let keys = ["55".repeat(32), "66".repeat(32)];
    for round in 0..2 {
        let url = fresh(&format!("race{round}")).await;
        let mut nodes = Vec::new();
        for (i, k) in keys.iter().enumerate() {
            let d = dir(&format!("race{round}-{i}"));
            let env = [
                ("LOCUS_DATABASE_URL", url.as_str()),
                ("LOCUS_MASTER_KEY_SOURCE", "env"),
                ("LOCUS_MASTER_KEY", k.as_str()),
            ];
            let (child, url) = spawn(&plc, &d, &env, &d.with_extension("log"));
            // Stopped when dropped, also when an assertion fails.
            nodes.push((Node { child, url }, d));
        }
        let (mut serving, mut exited) = (Vec::new(), Vec::new());
        for _ in 0..240 {
            serving.clear();
            exited.clear();
            for (i, (n, _)) in nodes.iter_mut().enumerate() {
                if let Ok(Some(st)) = n.child.try_wait() {
                    assert!(!st.success());
                    exited.push(i);
                } else if up(&n.url).await {
                    serving.push(i);
                }
            }
            if serving.len() + exited.len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        assert_eq!((serving.len(), exited.len()), (1, 1), "round {round}");
        let (w, l) = (serving[0], exited[0]);
        let winner = MasterKey::from_hex(&keys[w], SourceKind::Env).unwrap();
        let loser = MasterKey::from_hex(&keys[l], SourceKind::Env).unwrap();
        let log = std::fs::read_to_string(nodes[l].1.with_extension("log")).unwrap();
        assert!(log.contains("master key"), "{log}");
        let p = sqlx::PgPool::connect(&url).await.unwrap();
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM master_key_check")
            .fetch_one(&p)
            .await
            .unwrap();
        p.close().await;
        assert_eq!(rows, 1);
        let s = sealed_pg(&url).await;
        assert!(s.checks(&winner) && !s.checks(&loser));
        assert!(!s.secrets.is_empty() && s.opens_under(&winner));
        assert!(
            s.secrets
                .iter()
                .all(|(n, x)| loser.open(format!("secret:{n}").as_bytes(), x).is_err()),
            "a secret under the losing key"
        );

        if round == 0 {
            // A third node with the losing key: refused, writing nothing.
            let (u, wd) = (nodes[w].0.url.clone(), nodes[w].1.clone());
            let (did, token) = account(&u, "pat.test").await;
            let before = sealed_pg(&url).await;
            let d = dir("late");
            let env = [
                ("LOCUS_DATABASE_URL", url.as_str()),
                ("LOCUS_MASTER_KEY_SOURCE", "env"),
                ("LOCUS_MASTER_KEY", keys[l].as_str()),
            ];
            let log = refused(&plc, &d, &env).await;
            assert!(log.contains("not the install's key"), "{log}");
            assert_eq!(before, sealed_pg(&url).await);
            post(&u, &did, &token, "the winner still signs").await;

            // An offline command while a node runs on the database: refused.
            let env = [
                ("LOCUS_DATABASE_URL", url.as_str()),
                ("LOCUS_MASTER_KEY_SOURCE", "env"),
                ("LOCUS_MASTER_KEY", keys[w].as_str()),
            ];
            let out = run(
                &plc,
                &wd,
                &env,
                &[
                    "keys",
                    "rewrap",
                    "--to",
                    "env",
                    "--out",
                    "/nonexistent-dir/k",
                ],
            );
            assert!(!out.status.success());
            assert!(text(&out).contains("a node is running"), "{}", text(&out));
            assert_eq!(before, sealed_pg(&url).await);
        }
        drop(nodes);
    }
}
