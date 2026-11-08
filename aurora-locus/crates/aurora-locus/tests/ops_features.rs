//! Operator features with the real binary: direct TLS getting its
//! certificate from an ACME server (Pebble, in Docker, when
//! `LOCUS_TEST_PEBBLE_IMAGE` names its image) by TLS-ALPN-01 and HTTP-01,
//! and `--force` on the offline commands. A mock PLC directory runs in the
//! test.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::{Path as UrlPath, State};
use axum::routing::get;
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

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// The binary with a clean env, set up headlessly on first start.
fn command(plc: &str, dir: &Path, port: u16, extra: &[(&str, String)]) -> Command {
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
        .env("PDS_DID_PLC_URL", plc)
        .env("PDS_INVITE_REQUIRED", "false")
        .env("LOCUS_LOG_FORMAT", "json")
        .envs(extra.iter().map(|(k, v)| (*k, v.as_str())));
    c
}

fn run(plc: &str, dir: &Path, extra: &[(&str, String)], args: &[&str]) -> Output {
    command(plc, dir, free_port(), extra)
        .args(args)
        .output()
        .unwrap()
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
    log: PathBuf,
}

impl Node {
    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for Node {
    /// SIGTERM, so the node releases its slot and lock cleanly.
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

/// Starts a node and waits until `/xrpc/_health` answers; with `setup`,
/// until headless setup is done too.
async fn start(plc: &str, dir: &Path, extra: &[(&str, String)], setup: bool) -> Node {
    let port = free_port();
    let log = dir.with_extension("log");
    let file = std::fs::File::create(&log).unwrap();
    let mut cmd = command(plc, dir, port, extra);
    if !setup {
        cmd.env_remove("LOCUS_SETUP_AUTO");
    }
    let mut child = cmd
        .arg("serve")
        .stdout(Stdio::from(file.try_clone().unwrap()))
        .stderr(Stdio::from(file))
        .spawn()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}");
    let path = if setup {
        "/xrpc/com.atproto.server.describeServer"
    } else {
        "/xrpc/_health"
    };
    for _ in 0..240 {
        if let Ok(r) = reqwest::get(format!("{url}{path}")).await
            && r.status().is_success()
        {
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

// ------------------------------------------------------------ Pebble

/// A Pebble ACME server in Docker, on the host network so it reaches the
/// node's ports; `pds.test` resolves to 127.0.0.1 inside it.
struct Pebble {
    name: String,
    _dir: tempfile::TempDir,
    directory: String,
    mgmt: String,
    /// The CA of Pebble's own HTTPS certificate (`LOCUS_ACME_CA`).
    ca: PathBuf,
    ca_pem: String,
}

impl Drop for Pebble {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.name])
            .output();
    }
}

impl Pebble {
    fn client(&self) -> reqwest::Client {
        // Pebble answers 400 to a request with no User-Agent.
        reqwest::Client::builder()
            .add_root_certificate(reqwest::Certificate::from_pem(self.ca_pem.as_bytes()).unwrap())
            .user_agent("locus-test")
            .build()
            .unwrap()
    }

    /// The root Pebble issues under (made fresh at each start).
    async fn root(&self) -> reqwest::Certificate {
        let pem = self
            .client()
            .get(format!("{}/roots/0", self.mgmt))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        reqwest::Certificate::from_pem(pem.as_bytes()).unwrap()
    }
}

/// Starts Pebble validating TLS-ALPN-01 on `tls_port` and HTTP-01 on
/// `http_port`, with no shortcuts (`PEBBLE_VA_ALWAYS_VALID=0`). None
/// when no image is configured.
async fn pebble(tls_port: u16, http_port: u16) -> Option<Pebble> {
    let Ok(image) = std::env::var("LOCUS_TEST_PEBBLE_IMAGE") else {
        eprintln!("LOCUS_TEST_PEBBLE_IMAGE isn't set; skipping");
        return None;
    };
    let dir = tempfile::tempdir().unwrap();
    // Pebble's HTTPS certificate, from a CA of the test's own.
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "locus test ACME CA");
    let ca = rcgen::CertifiedIssuer::self_signed(ca_params, ca_key).unwrap();
    let ca_pem = AsRef::<rcgen::Certificate>::as_ref(&ca).pem();
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["localhost".into(), "127.0.0.1".into()])
        .unwrap()
        .signed_by(&key, &ca)
        .unwrap();
    std::fs::write(dir.path().join("cert.pem"), cert.pem()).unwrap();
    std::fs::write(dir.path().join("key.pem"), key.serialize_pem()).unwrap();
    let ca_path = dir.path().join("ca.pem");
    std::fs::write(&ca_path, &ca_pem).unwrap();
    let (acme_port, mgmt_port) = (free_port(), free_port());
    let config = json!({"pebble": {
        "listenAddress": format!("127.0.0.1:{acme_port}"),
        "managementListenAddress": format!("127.0.0.1:{mgmt_port}"),
        "certificate": "/cfg/cert.pem",
        "privateKey": "/cfg/key.pem",
        "httpPort": http_port,
        "tlsPort": tls_port,
        "ocspResponderURL": "",
        "externalAccountBindingRequired": false,
        "retryAfter": {"authz": 1, "order": 1},
        "keyAlgorithm": "ecdsa",
        "profiles": {"default": {"description": "default", "validityPeriod": 7_776_000}},
    }});
    std::fs::write(dir.path().join("pebble.json"), config.to_string()).unwrap();
    for f in ["cert.pem", "key.pem", "pebble.json"] {
        let p = dir.path().join(f);
        let mut perm = std::fs::metadata(&p).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o644);
        std::fs::set_permissions(&p, perm).unwrap();
    }
    let mut perm = std::fs::metadata(dir.path()).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
    std::fs::set_permissions(dir.path(), perm).unwrap();
    let name = format!("locus-test-pebble-{}-{tls_port}", std::process::id());
    let out = Command::new("docker")
        .args(["run", "-d", "--rm", "--name", &name, "--network", "host"])
        .args(["--add-host", "pds.test:127.0.0.1"])
        .args([
            "-e",
            "PEBBLE_VA_ALWAYS_VALID=0",
            "-e",
            "PEBBLE_VA_NOSLEEP=1",
        ])
        .args([
            "-e",
            "PEBBLE_WFE_NONCEREJECT=0",
            "-e",
            "PEBBLE_AUTHZREUSE=0",
        ])
        .arg("-v")
        .arg(format!("{}:/cfg:ro", dir.path().display()))
        .args([&image, "-config", "/cfg/pebble.json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "docker run: {}", text(&out));
    let p = Pebble {
        name,
        _dir: dir,
        directory: format!("https://localhost:{acme_port}/dir"),
        mgmt: format!("https://localhost:{mgmt_port}"),
        ca: ca_path,
        ca_pem,
    };
    let mut last = String::new();
    for _ in 0..100 {
        match p.client().get(&p.directory).send().await {
            Ok(r) if r.status().is_success() => return Some(p),
            Ok(r) => last = format!("status {}", r.status()),
            Err(e) => last = format!("{e:?}"),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let logs = Command::new("docker")
        .args(["logs", &p.name])
        .output()
        .unwrap();
    panic!("pebble didn't come up ({last}): {}", text(&logs));
}

/// A node in setup mode serving HTTPS for `pds.test` on `tls_port`, its
/// certificates from `pebble`; `http` is its port-80 listener ("off" for
/// none).
async fn tls_node(plc: &str, dir: &Path, pebble: &Pebble, tls_port: u16, http: &str) -> Node {
    let extra = [
        ("PDS_HOSTNAME", "pds.test".to_owned()),
        ("PDS_SERVICE_HANDLE_DOMAINS", ".handles.test".to_owned()),
        ("LOCUS_TLS", "true".to_owned()),
        ("LOCUS_TLS_LISTEN", format!("127.0.0.1:{tls_port}")),
        ("LOCUS_TLS_HTTP_LISTEN", http.to_owned()),
        ("LOCUS_ACME_DIRECTORY", pebble.directory.clone()),
        ("LOCUS_ACME_CA", pebble.ca.display().to_string()),
        ("LOCUS_ACME_EMAIL", "ops@example.com".to_owned()),
        ("RUST_LOG", "info,locus_http::acme=debug".to_owned()),
    ];
    start(plc, dir, &extra, false).await
}

/// Waits for an HTTPS answer for `name` on `port` over a certificate that
/// chains to `root` (Pebble's) and names `name`.
async fn https_ok(name: &str, port: u16, root: &reqwest::Certificate, node: &Node) {
    let client = https_client(name, port, root);
    let mut last = String::new();
    for _ in 0..120 {
        match client
            .get(format!("https://{name}:{port}/xrpc/_health"))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => return,
            Ok(r) => last = format!("status {}", r.status()),
            Err(e) => last = format!("{e:?}"),
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("no certificate for {name}: {last}\n{}", node.log());
}

fn https_client(name: &str, port: u16, root: &reqwest::Certificate) -> reqwest::Client {
    reqwest::Client::builder()
        .tls_built_in_root_certs(false)
        .add_root_certificate(root.clone())
        .resolve(name, ([127, 0, 0, 1], port).into())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

/// Port 80 closed and off: the PDS hostname's certificate is still issued,
/// proved by TLS-ALPN-01 on the HTTPS port itself. A name `/tls-check`
/// doesn't approve still gets no certificate.
#[tokio::test(flavor = "multi_thread")]
async fn direct_tls_gets_its_certificate_by_tls_alpn_01_with_port_80_closed() {
    let tls_port = free_port();
    // Pebble's HTTP-01 port: nothing listens there.
    let Some(pebble) = pebble(tls_port, free_port()).await else {
        return;
    };
    let plc = plc().await;
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("a");
    let node = tls_node(&plc, &dir, &pebble, tls_port, "off").await;
    let ca = pebble.root().await;
    https_ok("pds.test", tls_port, &ca, &node).await;
    // Ordered once, by TLS-ALPN-01 only.
    let log = node.log();
    assert!(log.contains("certificate issued"), "{log}");
    assert!(!log.contains("Http01"), "{log}");
    assert!(log.contains("TLS-ALPN-01 validation handshake"), "{log}");
    // Stored in the cache.
    assert!(dir.join("tls/pds.test.pem").exists());
    assert!(dir.join("tls/pds.test.key").exists());

    // An unapproved name: the handshake fails, and no order is made.
    let other = https_client("other.test", tls_port, &ca);
    assert!(
        other
            .get(format!("https://other.test:{tls_port}/xrpc/_health"))
            .send()
            .await
            .is_err()
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!dir.join("tls/other.test.pem").exists());
    assert!(
        !node.log().contains("certificate order failed"),
        "{}",
        node.log()
    );
}

/// Port 80 served but unreachable for the CA: HTTP-01 fails, and the order
/// is retried by TLS-ALPN-01.
#[tokio::test(flavor = "multi_thread")]
async fn direct_tls_falls_back_to_tls_alpn_01_when_http_01_fails() {
    let tls_port = free_port();
    let Some(pebble) = pebble(tls_port, free_port()).await else {
        return;
    };
    let plc = plc().await;
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("a");
    let http = format!("127.0.0.1:{}", free_port());
    let node = tls_node(&plc, &dir, &pebble, tls_port, &http).await;
    let ca = pebble.root().await;
    https_ok("pds.test", tls_port, &ca, &node).await;
    let log = node.log();
    assert!(log.contains("Http01 failed"), "{log}");
    assert!(log.contains("certificate issued"), "{log}");
    assert!(log.contains("TLS-ALPN-01 validation handshake"), "{log}");
}

/// Port 80 reachable: HTTP-01, answered by the port-80 listener, is
/// enough; TLS-ALPN-01 isn't needed.
#[tokio::test(flavor = "multi_thread")]
async fn direct_tls_uses_http_01_when_port_80_answers() {
    let (tls_port, http_port) = (free_port(), free_port());
    let Some(pebble) = pebble(tls_port, http_port).await else {
        return;
    };
    let plc = plc().await;
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("a");
    let http = format!("127.0.0.1:{http_port}");
    let node = tls_node(&plc, &dir, &pebble, tls_port, &http).await;
    let ca = pebble.root().await;
    https_ok("pds.test", tls_port, &ca, &node).await;
    let log = node.log();
    assert!(log.contains("certificate issued"), "{log}");
    assert!(!log.contains("Http01 failed"), "{log}");
    assert!(!log.contains("TLS-ALPN-01 validation handshake"), "{log}");
    // Port 80 still redirects everything else to HTTPS.
    let r = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(format!("http://{http}/xrpc/_health"))
        .header("host", "pds.test")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 308);
    assert_eq!(
        r.headers()["location"].to_str().unwrap(),
        "https://pds.test/xrpc/_health"
    );
}

// ------------------------------------------------------------ --force

/// Postgres: a fresh database's URL, or None without a test server.
async fn fresh_pg(tag: &str) -> Option<String> {
    let base = std::env::var("LOCUS_TEST_POSTGRES_URL").ok()?;
    let admin = sqlx::PgPool::connect(&base).await.unwrap();
    let name = format!("locus_test_force_{tag}_{}", std::process::id());
    sqlx::query(&format!("DROP DATABASE IF EXISTS {name}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&admin)
        .await
        .unwrap();
    let (head, _) = base.rsplit_once('/').unwrap();
    Some(format!("{head}/{name}"))
}

/// While a node holds a SQLite install: every offline command is refused;
/// with `--force`, `migrate storage` and `import reference-pds` run (with
/// a warning), and `keys rewrap`, `backup restore` and `backup
/// post-restore` are still refused, as they are with no node running.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread")]
async fn force_runs_the_forceable_offline_commands_past_a_running_node() {
    let plc = plc().await;
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("a");
    // A key from env, so `migrate storage` can run.
    let extra = [
        ("LOCUS_MASTER_KEY_SOURCE", "env".to_owned()),
        ("LOCUS_MASTER_KEY", "44".repeat(32)),
    ];
    let node = start(&plc, &dir, &extra, true).await;
    let r = reqwest::Client::new()
        .post(format!(
            "{}/xrpc/com.atproto.server.createAccount",
            node.url
        ))
        .json(
            &json!({"handle": "pat.test", "email": "pat@example.com", "password": "pw-123456789"}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    let pg = fresh_pg("sqlite").await;
    // Without Postgres, a URL nothing answers: past the lock, it fails to
    // connect instead.
    let url = pg
        .clone()
        .unwrap_or_else(|| format!("postgres://locus:locus@127.0.0.1:{}/x", free_port()));
    let migrate = ["migrate", "storage", "--to", "postgres", "--url", &url];

    // Unforced: refused, nothing copied.
    let out = run(&plc, &dir, &extra, &migrate);
    assert!(!out.status.success());
    assert!(text(&out).contains("a node is running"), "{}", text(&out));
    if let Some(pg) = &pg {
        let db = sqlx::PgPool::connect(pg).await.unwrap();
        let tables: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(tables, 0);
    }

    // Forced: a warning, then it runs.
    let mut forced = migrate.to_vec();
    forced.push("--force");
    let out = run(&plc, &dir, &extra, &forced);
    let t = text(&out);
    assert!(t.contains("Warning: a node is running"), "{t}");
    assert!(t.contains("--force"), "{t}");
    assert!(!t.contains("stop every node first"), "{t}");
    if let Some(pg) = &pg {
        assert!(out.status.success(), "{t}");
        let db = sqlx::PgPool::connect(pg).await.unwrap();
        let handle: String = sqlx::query_scalar("SELECT handle FROM account")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(handle, "pat.test");
    } else {
        assert!(!out.status.success());
    }

    // import reference-pds: refused; forced, it gets past the lock (and
    // stops at its own checks: there's no pds.env).
    let missing = root.path().join("missing.env");
    let import = [
        "import",
        "reference-pds",
        root.path().to_str().unwrap(),
        "--env",
        missing.to_str().unwrap(),
    ];
    let out = run(&plc, &dir, &extra, &import);
    assert!(!out.status.success());
    assert!(text(&out).contains("a node is running"), "{}", text(&out));
    let mut forced = import.to_vec();
    forced.push("--force");
    let out = run(&plc, &dir, &extra, &forced);
    let t = text(&out);
    assert!(!out.status.success());
    assert!(t.contains("Warning: a node is running"), "{t}");
    assert!(t.contains("can't read"), "{t}");

    // The three never-forced commands: refused with and without --force.
    let key_out = root.path().join("new.key");
    let backup = root.path().join("backup");
    let never: [(&str, Vec<&str>); 3] = [
        (
            "keys rewrap",
            vec![
                "keys",
                "rewrap",
                "--to",
                "env",
                "--out",
                key_out.to_str().unwrap(),
            ],
        ),
        (
            "backup restore",
            vec![
                "backup",
                "restore",
                backup.to_str().unwrap(),
                "--backend",
                "disk",
            ],
        ),
        ("backup post-restore", vec!["backup", "post-restore"]),
    ];
    for (name, args) in &never {
        let out = run(&plc, &dir, &extra, args);
        assert!(!out.status.success(), "{name}");
        assert!(text(&out).contains("node"), "{name}: {}", text(&out));
        let mut forced = args.clone();
        forced.push("--force");
        let out = run(&plc, &dir, &extra, &forced);
        let t = text(&out);
        assert!(!out.status.success(), "{name}: {t}");
        assert!(
            t.contains(&format!("`{name}` can never be forced")),
            "{name}: {t}"
        );
        assert!(!t.contains("Warning"), "{name}: {t}");
    }
    assert!(!key_out.exists());

    // With the node stopped, --force on them is refused all the same.
    drop(node);
    for (name, args) in &never {
        let mut forced = args.clone();
        forced.push("--force");
        let out = run(&plc, &dir, &extra, &forced);
        let t = text(&out);
        assert!(!out.status.success(), "{name}: {t}");
        assert!(
            t.contains(&format!("`{name}` can never be forced")),
            "{name}: {t}"
        );
    }
    assert!(!key_out.exists());
}

/// Postgres: a node's shared advisory lock keeps offline commands out,
/// and `--force` gets the forceable ones past it.
#[tokio::test(flavor = "multi_thread")]
async fn force_gets_past_a_postgres_node() {
    let Some(pg) = fresh_pg("pg").await else {
        return;
    };
    let plc = plc().await;
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("a");
    let extra = [
        ("LOCUS_DATABASE_URL", pg.clone()),
        ("LOCUS_MASTER_KEY_SOURCE", "env".to_owned()),
        ("LOCUS_MASTER_KEY", "33".repeat(32)),
    ];
    let node = start(&plc, &dir, &extra, true).await;
    let missing = root.path().join("missing.env");
    let import = [
        "import",
        "reference-pds",
        root.path().to_str().unwrap(),
        "--env",
        missing.to_str().unwrap(),
    ];
    let migrate = ["migrate", "storage", "--to", "postgres", "--url", &pg];
    for args in [&import[..], &migrate[..]] {
        let out = run(&plc, &dir, &extra, args);
        assert!(!out.status.success());
        assert!(
            text(&out).contains("a node is running on this database"),
            "{}",
            text(&out)
        );
        // Forced: past the lock, to the command's own refusal (this
        // install is on Postgres).
        let mut forced = args.to_vec();
        forced.push("--force");
        let out = run(&plc, &dir, &extra, &forced);
        let t = text(&out);
        assert!(!out.status.success());
        assert!(t.contains("Warning: a node is running"), "{t}");
        assert!(t.to_lowercase().contains("postgres"), "{t}");
    }
    let out = run(
        &plc,
        &dir,
        &extra,
        &[
            "keys",
            "rewrap",
            "--to",
            "kms",
            "--kms-key-id",
            "x",
            "--force",
        ],
    );
    assert!(
        text(&out).contains("`keys rewrap` can never be forced"),
        "{}",
        text(&out)
    );
    drop(node);
}
