//! The operator CLI's online commands: each is a call to the admin API,
//! over the admin socket (as the owner; root or the service's own user)
//! or over HTTP with an operator API token (`--url`, `--token`).

use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const NS: &str = "app.nearhorizon.locus";

/// How the CLI reaches a node.
pub enum Client {
    Socket(PathBuf),
    Http { url: String, token: String },
}

fn query_string(q: &[(String, String)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(q.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .finish()
}

/// A chunked HTTP/1.1 body, decoded.
fn dechunk(mut body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let Some(nl) = body.windows(2).position(|w| w == b"\r\n") else {
            return out;
        };
        let size = std::str::from_utf8(&body[..nl])
            .ok()
            .and_then(|s| usize::from_str_radix(s.split(';').next().unwrap_or("").trim(), 16).ok())
            .unwrap_or(0);
        body = &body[nl + 2..];
        if size == 0 || body.len() < size {
            return out;
        }
        out.extend_from_slice(&body[..size]);
        body = body.get(size + 2..).unwrap_or(&[]);
    }
}

impl Client {
    /// Calls `nsid`: a query (GET with `query`) or, with a body, a
    /// procedure (POST). Returns the JSON answer, or the error's message.
    pub async fn call(
        &self,
        nsid: &str,
        query: &[(String, String)],
        body: Option<Value>,
    ) -> Result<Value, String> {
        let (status, bytes) = self.call_raw(nsid, query, body).await?;
        let v = answer_json(&bytes);
        if (200..300).contains(&status) {
            Ok(v)
        } else {
            Err(answer_error(status, &v))
        }
    }

    /// Calls `nsid` and returns the answer's status and bytes as they are.
    pub async fn call_raw(
        &self,
        nsid: &str,
        query: &[(String, String)],
        body: Option<Value>,
    ) -> Result<(u16, Vec<u8>), String> {
        let body = body.map(|b| (b.to_string().into_bytes(), "application/json"));
        self.send(nsid, query, body).await
    }

    /// Calls `nsid` as a procedure with a raw body of `content_type`.
    pub async fn call_bytes(
        &self,
        nsid: &str,
        bytes: Vec<u8>,
        content_type: &'static str,
    ) -> Result<Value, String> {
        let (status, out) = self.send(nsid, &[], Some((bytes, content_type))).await?;
        let v = answer_json(&out);
        if (200..300).contains(&status) {
            Ok(v)
        } else {
            Err(answer_error(status, &v))
        }
    }

    async fn send(
        &self,
        nsid: &str,
        query: &[(String, String)],
        body: Option<(Vec<u8>, &'static str)>,
    ) -> Result<(u16, Vec<u8>), String> {
        let path = if query.is_empty() {
            format!("/xrpc/{nsid}")
        } else {
            format!("/xrpc/{nsid}?{}", query_string(query))
        };
        let (status, bytes) = match self {
            Self::Http { url, token } => {
                let client = reqwest::Client::new();
                let url = format!("{}{path}", url.trim_end_matches('/'));
                let req = match body {
                    Some((b, ct)) => client
                        .post(url)
                        .header(reqwest::header::CONTENT_TYPE, ct)
                        .body(b),
                    None => client.get(url),
                };
                let res = req
                    .bearer_auth(token)
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                let status = res.status().as_u16();
                (
                    status,
                    res.bytes().await.map_err(|e| e.to_string())?.to_vec(),
                )
            }
            Self::Socket(sock) => {
                let mut stream = tokio::net::UnixStream::connect(sock)
                    .await
                    .map_err(|e| format!("can't reach the admin socket {}: {e} (is a node running? or use --url and --token)", sock.display()))?;
                let (method, payload, ct) = match body {
                    Some((b, ct)) => ("POST", b, ct),
                    None => ("GET", Vec::new(), "application/json"),
                };
                let head = format!(
                    "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: {ct}\r\nContent-Length: {}\r\n\r\n",
                    payload.len()
                );
                stream
                    .write_all(head.as_bytes())
                    .await
                    .map_err(|e| e.to_string())?;
                stream
                    .write_all(&payload)
                    .await
                    .map_err(|e| e.to_string())?;
                let mut raw = Vec::new();
                stream
                    .read_to_end(&mut raw)
                    .await
                    .map_err(|e| e.to_string())?;
                let split = raw
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .ok_or("a broken answer from the socket")?;
                let headers = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
                let status = headers
                    .split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse::<u16>().ok())
                    .ok_or("a broken answer from the socket")?;
                let body = &raw[split + 4..];
                let body = if headers.contains("transfer-encoding: chunked") {
                    dechunk(body)
                } else {
                    body.to_vec()
                };
                (status, body)
            }
        };
        Ok((status, bytes))
    }
}

fn answer_json(bytes: &[u8]) -> Value {
    if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(bytes).into_owned()))
    }
}

fn answer_error(status: u16, v: &Value) -> String {
    match (v["error"].as_str(), v["message"].as_str()) {
        (Some(e), Some(m)) => format!("{e}: {m}"),
        (_, Some(m)) => m.to_owned(),
        _ => format!("HTTP {status}: {v}"),
    }
}

#[derive(Subcommand)]
pub enum AccountCmd {
    /// Accounts by handle, email or DID (or their start).
    List {
        query: Option<String>,
    },
    /// One account and its takedown status.
    Show {
        did: String,
    },
    /// A new account (the password is asked for).
    Create {
        handle: String,
        email: String,
    },
    /// Deletes an account and everything it holds.
    Delete {
        did: String,
        #[arg(long)]
        yes: bool,
    },
    Takedown {
        did: String,
        #[arg(long = "ref")]
        reference: Option<String>,
    },
    Untakedown {
        did: String,
    },
    /// Deactivates the account (its owner can reactivate it).
    Suspend {
        did: String,
    },
    Unsuspend {
        did: String,
    },
    /// A new password (asked for); every sign-in of the account ends.
    ResetPassword {
        did: String,
    },
    SetHandle {
        did: String,
        handle: String,
    },
    SetEmail {
        did: String,
        email: String,
    },
    /// Blocks or allows private posts for the account here.
    PrivateTier {
        did: String,
        state: BlockState,
    },
    /// The account's latest import.
    ImportStatus {
        did: String,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum BlockState {
    Block,
    Unblock,
}

#[derive(Subcommand)]
pub enum InviteCmd {
    Create {
        #[arg(long, default_value_t = 1)]
        count: i64,
        #[arg(long, default_value_t = 1)]
        uses: i64,
    },
    List {
        #[arg(long)]
        by_usage: bool,
    },
    Disable {
        code: String,
    },
}

#[derive(Subcommand)]
pub enum SettingsCmd {
    /// One setting, or every one.
    Get {
        key: Option<String>,
    },
    /// A value as JSON (`true`, `30`, `["a"]`), or plain text.
    Set {
        key: String,
        value: String,
    },
    List,
    /// Every setting as one JSON object.
    Export,
    /// Sets every setting in a JSON object (as `export` writes).
    Import {
        file: PathBuf,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub enum OperatorCmd {
    /// A new operator (the password is asked for).
    Add {
        username: String,
        #[arg(long, default_value = "viewer")]
        role: String,
    },
    /// Disables an operator.
    Remove {
        username: String,
        #[arg(long)]
        yes: bool,
    },
    List,
    ResetPassword {
        username: String,
    },
    SetRole {
        username: String,
        role: String,
    },
}

#[derive(Subcommand)]
pub enum ModerationCmd {
    /// A DID, an at:// URI, or a DID with --blob.
    Status {
        subject: String,
        #[arg(long)]
        blob: Option<String>,
    },
    Takedown {
        subject: String,
        #[arg(long)]
        blob: Option<String>,
        #[arg(long = "ref")]
        reference: Option<String>,
    },
    Restore {
        subject: String,
        #[arg(long)]
        blob: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum CrawlCmd {
    /// Asks every relay to crawl now.
    Request,
}

#[derive(Subcommand)]
pub enum JobsCmd {
    List,
}

#[derive(Subcommand)]
pub enum KeysCmd {
    /// The held keys, the last key backup, and how accounts' documents
    /// list the PLC rotation keys.
    Show,
    /// An encrypted backup of every key (the passphrase is asked for).
    Backup {
        /// Where to write it (created, mode 0600; never overwritten).
        #[arg(long)]
        out: PathBuf,
        /// Read the passphrase from this file (or `-` for stdin) instead of
        /// asking, e.g. for scheduled backups.
        #[arg(long)]
        passphrase_file: Option<PathBuf>,
    },
    #[command(subcommand)]
    Set(KeysSetCmd),
    /// A new key for the service's own DID document.
    RotateServiceKey {
        #[arg(long)]
        yes: bool,
    },
    /// The reference's rotate-keys: points each account's PLC document at
    /// the signing key stored here, then announces #identity and a fresh
    /// commit's #sync.
    RotateAccount {
        dids: Vec<String>,
        /// A file of DIDs, one per line (did:plc ones are used).
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Reads a key backup: adds the PLC rotation keys this PDS lacks, and
    /// restores account signing keys their live DID documents list.
    Import {
        backup: PathBuf,
        #[arg(long)]
        passphrase_file: Option<PathBuf>,
        #[arg(long)]
        yes: bool,
    },
    /// Changes the master key's source (offline: stop every node first).
    /// Every secret is re-encrypted under the new key in one transaction.
    Rewrap {
        #[arg(long, value_enum)]
        to: RewrapTo,
        /// The new key file (`--to file`); a new path.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Where to write the new key (`--to env`), to move into
        /// LOCUS_MASTER_KEY.
        #[arg(long)]
        out: Option<PathBuf>,
        /// The KMS key (`--to kms`).
        #[arg(long)]
        kms_key_id: Option<String>,
    },
    /// Forgets a secondary PLC rotation key no account's document still
    /// needs (each document that may list it is checked first).
    DropPlcKey {
        did_key: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum RewrapTo {
    File,
    Env,
    Kms,
}

#[derive(Subcommand)]
pub enum MigrateCmd {
    /// Moves every blob to another backend while the node keeps serving,
    /// then cuts over with a short pause.
    Blobs {
        #[arg(long, value_enum)]
        to: Option<crate::restore::BackendKind>,
        /// The disk store's root (`--to disk`; default the data dir).
        #[arg(long)]
        disk_root: Option<String>,
        /// Ends the running migration without cutting over.
        #[arg(long)]
        cancel: bool,
        /// Where the migration is.
        #[arg(long)]
        status: bool,
        /// Goes ahead without the objects missing from the source.
        #[arg(long)]
        accept_missing: bool,
    },
    /// Moves the PDS to a new hostname: `<new>` prepares it (reserved,
    /// checked), `--confirm` starts the migration, `--cancel` clears a
    /// prepared one, `--close` ends a running one after its window.
    Hostname {
        new: Option<String>,
        #[arg(long)]
        confirm: bool,
        #[arg(long)]
        cancel: bool,
        #[arg(long)]
        close: bool,
        #[arg(long)]
        status: bool,
        /// Dev mode only: confirm without the network pre-checks.
        #[arg(long, hide = true)]
        skip_checks: bool,
        #[arg(long)]
        yes: bool,
    },
    /// Copies this SQLite install into an empty Postgres database (offline).
    Storage {
        #[arg(long, value_parser = ["postgres"])]
        to: String,
        #[arg(long)]
        url: String,
    },
}

#[derive(Subcommand)]
pub enum BackupCmd {
    /// A backup of a SQLite install, made by the running node: the
    /// database, every object it names, and a key backup.
    Create {
        /// Read the key backup's passphrase from this file (or `-` for
        /// stdin) instead of asking.
        #[arg(long)]
        passphrase_file: Option<PathBuf>,
        /// No key backup (the database's secrets still need the master key).
        #[arg(long)]
        no_keys: bool,
        /// Wait until it's done.
        #[arg(long)]
        wait: bool,
    },
    /// Backups made, newest first.
    List,
    /// Restores a backup into an empty install (offline).
    Restore {
        dir: PathBuf,
        /// Where the objects go.
        #[arg(long, value_enum)]
        backend: crate::restore::BackendKind,
        /// The disk store's root (`--backend disk`; default the data dir).
        #[arg(long)]
        disk_root: Option<PathBuf>,
        /// Restore a backup that failed (objects may be missing).
        #[arg(long)]
        accept_failed: bool,
        #[arg(long)]
        seq_floor: Option<i64>,
        /// A fresh commit and #sync per active account.
        #[arg(long)]
        emit_sync: bool,
        /// Shard role: the entryway's current token verify key (a file,
        /// or `-` for stdin). Required there.
        #[arg(long)]
        entryway_verify_key: Option<PathBuf>,
        /// Shard role: the entryway's current PLC rotation key (a file,
        /// or `-`). Required there.
        #[arg(long)]
        entryway_plc_rotation_key: Option<PathBuf>,
    },
    /// After a Postgres point-in-time recovery, before any node starts
    /// (offline).
    PostRestore {
        #[arg(long, value_enum)]
        backend: Option<crate::restore::BackendKind>,
        #[arg(long)]
        disk_root: Option<PathBuf>,
        #[arg(long)]
        accept_missing_majority: bool,
        #[arg(long)]
        seq_floor: Option<i64>,
        #[arg(long)]
        emit_sync: bool,
        /// Shard role: the entryway's current token verify key (a file,
        /// or `-` for stdin). Required there.
        #[arg(long)]
        entryway_verify_key: Option<PathBuf>,
        /// Shard role: the entryway's current PLC rotation key (a file,
        /// or `-`). Required there.
        #[arg(long)]
        entryway_plc_rotation_key: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum IdentityCmd {
    /// The reference's publish-identity: an #identity event for each DID.
    Publish {
        dids: Vec<String>,
        /// A file of DIDs, one per line.
        #[arg(long)]
        file: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum KeysSetCmd {
    /// The secret legacy sessions are signed with (signs every legacy
    /// session out). Read from a file, or `-` for stdin.
    JwtSecret {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// The passphrase scheduled backups encrypt their key backup with.
    BackupPassphrase {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// The DPoP nonce secret (64 hex characters; clients retry once).
    DpopSecret {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// Shard role: the entryway's token verify key (K256, hex or
    /// did:key). The previous one stays accepted 2 hours unless
    /// --revoke-previous.
    EntrywayVerifyKey {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        revoke_previous: bool,
        #[arg(long)]
        yes: bool,
    },
    /// Shard role: the entryway's PLC rotation key (genesis operations
    /// may be signed by it). Earlier ones stay accepted until a later
    /// --revoke-previous.
    EntrywayPlcRotationKey {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        revoke_previous: bool,
        #[arg(long)]
        yes: bool,
    },
    /// Shard role: the admin token this shard uses toward the entryway.
    EntrywayAdminToken {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// A new PLC rotation key for the PDS. It's stored pending and becomes
    /// current once a key backup holds it (or with
    /// --acknowledge-no-backup); the previous key keeps signing for the
    /// accounts whose documents list it.
    PlcRotationKey {
        /// Generate the new key here.
        #[arg(long)]
        generate: bool,
        /// Read the private key (hex) from this file, or `-` for stdin.
        #[arg(long)]
        file: Option<PathBuf>,
        /// Or a secp256k1 key in KMS (ECC_SECG_P256K1), which signs there.
        #[arg(long)]
        kms_key_id: Option<String>,
        /// Make the pending key current without a key backup.
        #[arg(long)]
        acknowledge_no_backup: bool,
        /// Remove the pending key.
        #[arg(long)]
        discard_pending: bool,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub enum KryphocronCmd {
    RotateNow,
    Status,
    Audit {
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
}

/// The online commands.
#[derive(Subcommand)]
pub enum AdminCmd {
    /// Version, nodes, accounts and jobs.
    Status,
    #[command(subcommand)]
    Account(AccountCmd),
    #[command(subcommand)]
    Invite(InviteCmd),
    #[command(subcommand)]
    Settings(SettingsCmd),
    #[command(subcommand)]
    Operator(OperatorCmd),
    #[command(subcommand)]
    Moderation(ModerationCmd),
    #[command(subcommand)]
    Crawl(CrawlCmd),
    #[command(subcommand)]
    Jobs(JobsCmd),
    #[command(subcommand)]
    Kryphocron(KryphocronCmd),
    #[command(subcommand)]
    Keys(KeysCmd),
    #[command(subcommand)]
    Identity(IdentityCmd),
    #[command(subcommand)]
    Backup(BackupCmd),
    #[command(subcommand)]
    Migrate(MigrateCmd),
    /// Any XRPC method: `key=value` query parameters, or `--body` JSON to
    /// call it as a procedure.
    Call {
        nsid: String,
        params: Vec<String>,
        #[arg(long)]
        body: Option<String>,
    },
}

fn q(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn print(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

fn secret(prompt: &str) -> Result<String, String> {
    let a = rpassword::prompt_password(format!("{prompt}: ")).map_err(|e| e.to_string())?;
    let b = rpassword::prompt_password("Again: ").map_err(|e| e.to_string())?;
    if a != b {
        return Err("The two differ.".into());
    }
    Ok(a)
}

/// The socket CLI's re-confirmation: a typed `yes` at a terminal.
fn confirm(what: &str, yes: bool) -> Result<(), String> {
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        return Err(format!(
            "{what}: run it at a terminal to confirm, or pass --yes"
        ));
    }
    print!("{what}. Type yes to go on: ");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    if line.trim() == "yes" {
        Ok(())
    } else {
        Err("Not confirmed.".into())
    }
}

fn subject(s: &str, blob: Option<&str>) -> Value {
    if s.starts_with("at://") {
        json!({"$type": "com.atproto.repo.strongRef", "uri": s, "cid": ""})
    } else if let Some(cid) = blob {
        json!({"$type": "com.atproto.admin.defs#repoBlobRef", "did": s, "cid": cid})
    } else {
        json!({"$type": "com.atproto.admin.defs#repoRef", "did": s})
    }
}

fn subject_query(s: &str, blob: Option<&str>) -> Vec<(String, String)> {
    if s.starts_with("at://") {
        q(&[("uri", s)])
    } else if let Some(cid) = blob {
        q(&[("did", s), ("blob", cid)])
    } else {
        q(&[("did", s)])
    }
}

/// Every setting's (key, value).
async fn settings(c: &Client) -> Result<Vec<(String, Value)>, String> {
    let all = c
        .call(&format!("{NS}.viewer.getSettings"), &[], None)
        .await?;
    Ok(all["settings"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|d| Some((d["key"].as_str()?.to_owned(), d["value"].clone())))
        .collect())
}

/// A setting's value from the command line: JSON, else plain text.
fn setting_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()))
}

/// Re-auth for a key change: over the socket, a typed confirmation; over
/// HTTP, the operator's password and code.
fn reauth(c: &Client, what: &str, yes: bool) -> Result<Value, String> {
    match c {
        Client::Socket(_) => {
            confirm(what, yes)?;
            Ok(json!({}))
        }
        Client::Http { .. } => {
            let password = rpassword::prompt_password("Your operator password: ")
                .map_err(|e| e.to_string())?;
            print!("Your code (empty without a code app): ");
            std::io::stdout().flush().map_err(|e| e.to_string())?;
            let mut code = String::new();
            std::io::stdin()
                .read_line(&mut code)
                .map_err(|e| e.to_string())?;
            Ok(json!({"password": password, "code": code.trim()}))
        }
    }
}

fn merge(mut a: Value, b: Value) -> Value {
    if let (Some(a), Value::Object(b)) = (a.as_object_mut(), b) {
        a.extend(b);
    }
    a
}

/// A private key from a file or stdin (`-`), never the command line.
fn read_key(path: &std::path::Path) -> Result<zeroize::Zeroizing<String>, String> {
    let text = if path.as_os_str() == "-" {
        let mut t = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut t).map_err(|e| e.to_string())?;
        t
    } else {
        std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?
    };
    Ok(zeroize::Zeroizing::new(text.trim().to_owned()))
}

/// Writes a new file only the owner can read; an existing one is refused.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(bytes)
        .and_then(|()| f.sync_all())
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// How long ago a time (ms since the epoch) was, roughly.
fn ago(at_ms: i64) -> String {
    let secs = (locus_core::time::now_ms() - at_ms).max(0) / 1000;
    match secs {
        0..60 => "just now".into(),
        60..3_600 => format!("{} minutes ago", secs / 60),
        3_600..86_400 => format!("{} hours ago", secs / 3_600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

/// DIDs from the command line and a file (one per line).
fn dids_from(
    mut dids: Vec<String>,
    file: Option<&std::path::Path>,
    plc_only: bool,
) -> Result<Vec<String>, String> {
    if let Some(f) = file {
        let text = std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?;
        dids.extend(
            text.lines()
                .map(str::trim)
                .filter(|l| {
                    if plc_only {
                        l.starts_with("did:plc:")
                    } else {
                        !l.is_empty()
                    }
                })
                .map(str::to_owned),
        );
    }
    if let Some(bad) = dids.iter().find(|d| !d.starts_with("did:")) {
        return Err(format!("{bad} isn't a DID"));
    }
    if dids.is_empty() {
        return Err("Give DIDs, or --file".into());
    }
    Ok(dids)
}

fn report_results(v: &Value) -> Result<(), String> {
    let mut failed = 0;
    for r in v["results"].as_array().cloned().unwrap_or_default() {
        let did = r["did"].as_str().unwrap_or("");
        if r["ok"] == json!(true) {
            let note = if r["plcUpdated"] == json!(true) {
                " (PLC document updated)"
            } else {
                ""
            };
            println!("{did}: done{note}");
        } else {
            failed += 1;
            println!("{did}: failed: {}", r["error"].as_str().unwrap_or("?"));
        }
    }
    if failed > 0 {
        Err(format!("{failed} failed"))
    } else {
        Ok(())
    }
}

async fn set_secret(
    c: &Client,
    name: &str,
    file: &std::path::Path,
    yes: bool,
    what: &str,
) -> Result<(), String> {
    let value = read_key(file)?;
    let body = merge(
        json!({"name": name, "value": *value}),
        reauth(c, what, yes)?,
    );
    let v = c
        .call(&format!("{NS}.owner.keys.setSecret"), &[], Some(body))
        .await?;
    println!(
        "{name} replaced (fingerprint {}). Every node picks it up within seconds.",
        v["fingerprint"].as_str().unwrap_or("")
    );
    Ok(())
}

async fn set_entryway(
    c: &Client,
    name: &str,
    file: &std::path::Path,
    revoke_previous: bool,
    yes: bool,
    what: &str,
) -> Result<(), String> {
    let value = read_key(file)?;
    let body = merge(
        json!({"name": name, "value": *value, "revokePrevious": revoke_previous}),
        reauth(c, what, yes)?,
    );
    let v = c
        .call(&format!("{NS}.owner.keys.setSecret"), &[], Some(body))
        .await?;
    println!(
        "{name} set (fingerprint {}). Every node picks it up within seconds.",
        v["fingerprint"].as_str().unwrap_or("")
    );
    if let Some(list) = v["accepted"].as_array() {
        for k in list {
            println!("  accepted: {}", k.as_str().unwrap_or(""));
        }
    } else if let Some(k) = v["accepted"].as_str() {
        println!("  accepted: {k}");
    }
    Ok(())
}

async fn keys(c: &Client, k: KeysCmd) -> Result<(), String> {
    let lx = |m: &str| format!("{NS}.{m}");
    match k {
        KeysCmd::Show => {
            let v = c.call(&lx("owner.keys.show"), &[], None).await?;
            println!(
                "master key   {} ({})",
                v["masterKey"]["id"].as_str().unwrap_or(""),
                v["masterKey"]["source"].as_str().unwrap_or("")
            );
            println!("service key  {}", v["serviceKey"].as_str().unwrap_or(""));
            for r in v["plcRotationKeys"].as_array().cloned().unwrap_or_default() {
                let note = if r["inLastBackup"] == json!(false) {
                    "  (not in a key backup yet)"
                } else {
                    ""
                };
                println!(
                    "PLC key      {} {}{note}",
                    r["didKey"].as_str().unwrap_or(""),
                    r["role"].as_str().unwrap_or("")
                );
            }
            match v["lastBackup"]["at"].as_i64() {
                Some(at) => println!(
                    "last backup  {} by {}",
                    ago(at),
                    v["lastBackup"]["by"].as_str().unwrap_or("?")
                ),
                None => println!("last backup  never: run `aurora-locus keys backup --out <file>`"),
            }
            let a = &v["accounts"];
            println!(
                "accounts     {} list the current key, {} only a secondary one, {} no held key, {} not checked yet",
                a["listingCurrent"],
                a["listingOnlySecondary"],
                a["listingNoHeldKey"],
                a["notChecked"]
            );
            Ok(())
        }
        KeysCmd::Backup {
            out,
            passphrase_file,
        } => {
            if out.exists() {
                return Err(format!("{} exists; choose a new file", out.display()));
            }
            let pass = match &passphrase_file {
                Some(f) => read_key(f)?,
                None => zeroize::Zeroizing::new(secret(
                    "A passphrase for the backup (at least 12 characters)",
                )?),
            };
            let (status, bytes) = c
                .call_raw(
                    &lx("owner.keys.backup"),
                    &[],
                    Some(json!({"passphrase": *pass})),
                )
                .await?;
            if !(200..300).contains(&status) {
                return Err(answer_error(status, &answer_json(&bytes)));
            }
            // Read it back before keeping it: a cut-off or damaged backup
            // fails here, not on the day it's needed.
            let (_, entries) = locus_core::key_backup::open(&bytes, &pass)
                .map_err(|e| format!("the backup didn't check out, nothing was written: {e}"))?;
            write_private(&out, &bytes)?;
            println!(
                "Wrote {} ({} keys and secrets). Keep it, and its passphrase, away from this server.",
                out.display(),
                entries.len()
            );
            Ok(())
        }
        KeysCmd::Set(KeysSetCmd::PlcRotationKey {
            generate,
            file,
            kms_key_id,
            acknowledge_no_backup,
            discard_pending,
            yes,
        }) => {
            let mut body = json!({
                "generate": generate,
                "acknowledgeNoBackup": acknowledge_no_backup,
                "discardPending": discard_pending,
            });
            if let Some(f) = &file {
                body["privateKeyHex"] = json!(*read_key(f)?);
            }
            if let Some(id) = &kms_key_id {
                body["kmsKeyId"] = json!(id);
            }
            let what = if discard_pending {
                "Remove the pending PLC rotation key"
            } else if acknowledge_no_backup {
                "Make the pending PLC rotation key current without a key backup that holds it"
            } else {
                "Store a new pending PLC rotation key for this PDS"
            };
            let body = merge(body, reauth(c, what, yes)?);
            let v = c
                .call(&lx("owner.keys.setPlcRotationKey"), &[], Some(body))
                .await?;
            println!(
                "{} is {}.",
                v["didKey"].as_str().unwrap_or(""),
                v["role"].as_str().unwrap_or("")
            );
            if let Some(next) = v["next"].as_str() {
                println!("{next}");
            }
            Ok(())
        }
        KeysCmd::Set(KeysSetCmd::JwtSecret { file, yes }) => {
            set_secret(
                c,
                "jwt-secret",
                &file,
                yes,
                "Replace the JWT secret (every legacy session signs out)",
            )
            .await
        }
        KeysCmd::Set(KeysSetCmd::BackupPassphrase { file, yes }) => {
            set_secret(
                c,
                "backup-passphrase",
                &file,
                yes,
                "Set the scheduled backups' key backup passphrase",
            )
            .await
        }
        KeysCmd::Set(KeysSetCmd::DpopSecret { file, yes }) => {
            set_secret(
                c,
                "dpop-secret",
                &file,
                yes,
                "Replace the DPoP nonce secret",
            )
            .await
        }
        KeysCmd::Set(KeysSetCmd::EntrywayVerifyKey {
            file,
            revoke_previous,
            yes,
        }) => {
            set_entryway(
                c,
                "entryway-verify-key",
                &file,
                revoke_previous,
                yes,
                "Set the entryway's token verify key",
            )
            .await
        }
        KeysCmd::Set(KeysSetCmd::EntrywayPlcRotationKey {
            file,
            revoke_previous,
            yes,
        }) => {
            set_entryway(
                c,
                "entryway-plc-rotation-key",
                &file,
                revoke_previous,
                yes,
                "Set the entryway's PLC rotation key",
            )
            .await
        }
        KeysCmd::Set(KeysSetCmd::EntrywayAdminToken { file, yes }) => {
            set_entryway(
                c,
                "entryway-admin-token",
                &file,
                false,
                yes,
                "Replace the admin token used toward the entryway",
            )
            .await
        }
        KeysCmd::RotateServiceKey { yes } => {
            let body = reauth(c, "Give the service DID a new signing key", yes)?;
            let v = c
                .call(&lx("owner.keys.rotateServiceKey"), &[], Some(body))
                .await?;
            println!(
                "The service key is now {}.",
                v["didKey"].as_str().unwrap_or("")
            );
            Ok(())
        }
        KeysCmd::RotateAccount { dids, file } => {
            let dids = dids_from(dids, file.as_deref(), true)?;
            for chunk in dids.chunks(50) {
                let v = c
                    .call(
                        &lx("admin.keys.rotateAccountKeys"),
                        &[],
                        Some(json!({"dids": chunk})),
                    )
                    .await?;
                report_results(&v)?;
            }
            Ok(())
        }
        KeysCmd::Import {
            backup,
            passphrase_file,
            yes,
        } => {
            let file = std::fs::read(&backup).map_err(|e| format!("{}: {e}", backup.display()))?;
            let header = locus_core::key_backup::header(&file).map_err(|e| e.to_string())?;
            println!(
                "A key backup of {}, made {}.",
                header.service_did,
                ago(header.created_at)
            );
            let pass = match &passphrase_file {
                Some(f) => read_key(f)?,
                None => zeroize::Zeroizing::new(
                    rpassword::prompt_password("Its passphrase: ").map_err(|e| e.to_string())?,
                ),
            };
            // Checked here first, so a wrong passphrase costs no round trip.
            locus_core::key_backup::open(&file, &pass).map_err(|e| e.to_string())?;
            let auth = reauth(c, "Import keys from this backup", yes)?;
            let mut head = merge(json!({"passphrase": *pass}), auth)
                .to_string()
                .into_bytes();
            head.push(b'\n');
            head.extend_from_slice(&file);
            let v = c
                .call_bytes(&lx("owner.keys.import"), head, "application/octet-stream")
                .await?;
            println!(
                "PLC rotation keys added: {}",
                v["plcKeysAdded"].as_array().map_or(0, Vec::len)
            );
            println!(
                "Account signing keys restored: {} (kept: {}, skipped: {})",
                v["signingKeysReplaced"].as_array().map_or(0, Vec::len),
                v["signingKeysKept"],
                v["skipped"]
            );
            for a in v["listNeitherKey"].as_array().cloned().unwrap_or_default() {
                println!(
                    "  lists neither key: {}{}",
                    a["did"].as_str().unwrap_or(""),
                    a["error"]
                        .as_str()
                        .map(|e| format!(" ({e})"))
                        .unwrap_or_default()
                );
            }
            if let Some(n) = v["next"].as_str() {
                println!("{n}");
            }
            Ok(())
        }
        KeysCmd::Rewrap { .. } => Err("keys rewrap runs offline, from main".into()),
        KeysCmd::DropPlcKey { did_key, yes } => {
            let body = merge(
                json!({"didKey": did_key}),
                reauth(c, &format!("Forget the PLC rotation key {did_key}"), yes)?,
            );
            c.call(&lx("owner.keys.dropPlcKey"), &[], Some(body))
                .await?;
            println!("{did_key} is no longer held.");
            Ok(())
        }
    }
}

/// Runs an online command.
pub async fn run(c: &Client, cmd: AdminCmd) -> Result<(), String> {
    let lx = |m: &str| format!("{NS}.{m}");
    match cmd {
        AdminCmd::Keys(k) => keys(c, k).await,
        AdminCmd::Backup(BackupCmd::Create {
            passphrase_file,
            no_keys,
            wait,
        }) => {
            let passphrase = if no_keys {
                None
            } else {
                Some(match &passphrase_file {
                    Some(f) => read_key(f)?,
                    None => zeroize::Zeroizing::new(secret(
                        "A passphrase for the key backup (at least 12 characters)",
                    )?),
                })
            };
            let v = c
                .call(
                    &lx("owner.backup.create"),
                    &[],
                    Some(json!({"passphrase": passphrase.as_deref().map(String::as_str)})),
                )
                .await?;
            let id = v["id"].as_str().unwrap_or("").to_owned();
            println!(
                "Backup {id} started, into {}.",
                v["dir"].as_str().unwrap_or("")
            );
            if wait {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    let l = c.call(&lx("viewer.backup.list"), &[], None).await?;
                    let run = l["backups"]
                        .as_array()
                        .and_then(|b| b.iter().find(|r| r["id"] == json!(id)).cloned())
                        .unwrap_or_default();
                    match run["status"].as_str() {
                        Some("running") => continue,
                        Some("done") => {
                            println!(
                                "Done: {} objects ({} missing), key backup: {}.",
                                run["objects"], run["missing"], run["keysBackup"]
                            );
                            return Ok(());
                        }
                        _ => {
                            return Err(format!(
                                "The backup failed: {}",
                                run["error"].as_str().unwrap_or("?")
                            ));
                        }
                    }
                }
            }
            Ok(())
        }
        AdminCmd::Migrate(MigrateCmd::Blobs {
            to,
            disk_root,
            cancel,
            status,
            accept_missing,
        }) => {
            if cancel {
                let v = c
                    .call(&lx("admin.migrateBlobs.cancel"), &[], Some(json!({})))
                    .await?;
                println!(
                    "Migration {} cancelled; objects it copied to the target are left there.",
                    v["id"].as_str().unwrap_or("")
                );
            } else if accept_missing {
                let v = c
                    .call(
                        &lx("admin.migrateBlobs.acceptMissing"),
                        &[],
                        Some(json!({})),
                    )
                    .await?;
                println!("{} missing objects acknowledged.", v["acknowledged"]);
            } else if let Some(to) = to {
                let to = match to {
                    crate::restore::BackendKind::Disk => "disk",
                    crate::restore::BackendKind::S3 => "s3",
                };
                let v = c
                    .call(
                        &lx("admin.migrateBlobs.start"),
                        &[],
                        Some(json!({"to": to, "diskRoot": disk_root})),
                    )
                    .await?;
                println!(
                    "Migration {} to {to} started; `aurora-locus migrate blobs --status` follows it.",
                    v["id"].as_str().unwrap_or("")
                );
            } else if status {
                let v = c.call(&lx("viewer.migrateBlobs.status"), &[], None).await?;
                let b = &v["backend"];
                println!(
                    "active backend {} (epoch {}{})",
                    b["active"].as_str().unwrap_or(""),
                    b["epoch"],
                    if b["paused"] == json!(true) {
                        ", paused"
                    } else {
                        ""
                    }
                );
                let m = &v["migration"];
                if m.is_null() {
                    println!("no migration");
                } else {
                    println!(
                        "migration {} to {}: {} after {} passes (copied through {}, verified through {})",
                        m["id"].as_str().unwrap_or(""),
                        m["to"].as_str().unwrap_or(""),
                        m["phase"].as_str().unwrap_or(""),
                        m["passes"],
                        m["copiedThrough"],
                        m["verifiedThrough"]
                    );
                    if let Some(n) = m["note"].as_str() {
                        println!("  {n}");
                    }
                }
                for k in v["missingSources"].as_array().cloned().unwrap_or_default() {
                    println!(
                        "  missing from the source: {}{}",
                        k["key"].as_str().unwrap_or(""),
                        if k["acknowledged"] == json!(true) {
                            " (acknowledged)"
                        } else {
                            ""
                        }
                    );
                }
            } else {
                return Err("give --to, --status, --cancel or --accept-missing".into());
            }
            Ok(())
        }
        AdminCmd::Migrate(MigrateCmd::Hostname {
            new,
            confirm,
            cancel,
            close,
            status,
            skip_checks,
            yes,
        }) => {
            if let Some(host) = new {
                let v = c
                    .call(
                        &lx("owner.hostname.prepare"),
                        &[],
                        Some(json!({"host": host})),
                    )
                    .await?;
                println!("{} is prepared.", v["host"].as_str().unwrap_or(""));
                if let Some(n) = v["next"].as_str() {
                    println!("{n}");
                }
            } else if confirm {
                let what = "Move this PDS to the prepared hostname: every OAuth session must sign in again, \
                            and it isn't reversible in practice";
                let body = merge(json!({"skipChecks": skip_checks}), reauth(c, what, yes)?);
                let v = c
                    .call(&lx("owner.hostname.confirm"), &[], Some(body))
                    .await?;
                println!("{} is the hostname now.", v["host"].as_str().unwrap_or(""));
                if let Some(n) = v["note"].as_str() {
                    println!("{n}");
                }
            } else if cancel {
                c.call(&lx("owner.hostname.cancel"), &[], Some(json!({})))
                    .await?;
                println!("The prepared hostname is cleared.");
            } else if close {
                let v = c
                    .call(&lx("owner.hostname.close"), &[], Some(json!({})))
                    .await?;
                println!(
                    "Closed, with {} accounts still pointing at the old hostname.",
                    v["stragglers"]
                );
            } else if status {
                let v = c.call(&lx("viewer.hostname.status"), &[], None).await?;
                println!("hostname {}", v["hostname"].as_str().unwrap_or(""));
                for h in v["oldHostsServed"].as_array().cloned().unwrap_or_default() {
                    println!("  still served in full: {}", h.as_str().unwrap_or(""));
                }
                if let Some(c) = v["candidate"].as_str() {
                    println!("  prepared: {c}");
                }
                for chk in v["prechecks"].as_array().cloned().unwrap_or_default() {
                    println!(
                        "    {} {}{}",
                        if chk["ok"] == json!(true) {
                            "ok  "
                        } else {
                            "FAIL"
                        },
                        chk["check"].as_str().unwrap_or(""),
                        chk["why"]
                            .as_str()
                            .filter(|w| !w.is_empty())
                            .map(|w| format!(": {w}"))
                            .unwrap_or_default()
                    );
                }
                for h in v["formerHosts"].as_array().cloned().unwrap_or_default() {
                    println!("  former: {}", h.as_str().unwrap_or(""));
                }
                for m in v["migrations"].as_array().cloned().unwrap_or_default() {
                    println!(
                        "migration {} → {}: {}",
                        m["from"].as_str().unwrap_or(""),
                        m["to"].as_str().unwrap_or(""),
                        m["state"].as_str().unwrap_or("")
                    );
                    for x in m["stragglers"].as_array().cloned().unwrap_or_default() {
                        println!(
                            "    still on the old host: {} ({})",
                            x["did"].as_str().unwrap_or(""),
                            x["reason"].as_str().unwrap_or("")
                        );
                    }
                }
            } else {
                return Err(
                    "give a new hostname, or --confirm, --cancel, --close or --status".into(),
                );
            }
            Ok(())
        }
        AdminCmd::Migrate(MigrateCmd::Storage { .. }) => {
            Err("migrate storage runs offline, from main".into())
        }
        AdminCmd::Backup(BackupCmd::List) => {
            let l = c.call(&lx("viewer.backup.list"), &[], None).await?;
            for r in l["backups"].as_array().cloned().unwrap_or_default() {
                println!(
                    "{}  {:<7}  {} objects, {} missing  {}",
                    r["id"].as_str().unwrap_or(""),
                    r["status"].as_str().unwrap_or(""),
                    r["objects"],
                    r["missing"],
                    r["dir"].as_str().unwrap_or("")
                );
            }
            Ok(())
        }
        AdminCmd::Backup(_) => Err("backup restore and post-restore run offline, from main".into()),
        AdminCmd::Identity(IdentityCmd::Publish { dids, file }) => {
            let dids = dids_from(dids, file.as_deref(), false)?;
            let v = c
                .call(
                    &lx("admin.identity.publish"),
                    &[],
                    Some(json!({"dids": dids})),
                )
                .await?;
            report_results(&v)
        }
        AdminCmd::Status => {
            let caps = c.call(&lx("describeCapabilities"), &[], None).await?;
            println!("aurora-locus {}", caps["version"].as_str().unwrap_or("?"));
            let jobs = c.call(&lx("viewer.listJobs"), &[], None).await?;
            for j in jobs["jobs"].as_array().cloned().unwrap_or_default() {
                let state = if j["lastError"].is_null() {
                    "ok".to_owned()
                } else {
                    format!("failing: {}", j["lastError"].as_str().unwrap_or(""))
                };
                println!("  job {:<24} {}", j["name"].as_str().unwrap_or(""), state);
            }
            let kr = c
                .call(&lx("viewer.kryphocron.getStatus"), &[], None)
                .await?;
            println!(
                "  private posts: {}",
                if kr["privateTierEnabled"].as_bool().unwrap_or(false) {
                    "on"
                } else {
                    "off"
                }
            );
            Ok(())
        }
        AdminCmd::Account(a) => account(c, a).await,
        AdminCmd::Invite(i) => match i {
            InviteCmd::Create { count, uses } => {
                print(
                    &c.call(
                        "com.atproto.server.createInviteCodes",
                        &[],
                        Some(json!({"codeCount": count, "useCount": uses})),
                    )
                    .await?,
                );
                Ok(())
            }
            InviteCmd::List { by_usage } => {
                print(
                    &c.call(
                        "com.atproto.admin.getInviteCodes",
                        &q(&[("sort", if by_usage { "usage" } else { "recent" })]),
                        None,
                    )
                    .await?,
                );
                Ok(())
            }
            InviteCmd::Disable { code } => {
                c.call(
                    "com.atproto.admin.disableInviteCodes",
                    &[],
                    Some(json!({"codes": [code]})),
                )
                .await?;
                println!("Disabled.");
                Ok(())
            }
        },
        AdminCmd::Settings(st) => match st {
            SettingsCmd::Get { key } => {
                let all = settings(c).await?;
                match key {
                    Some(k) => print(
                        &all.iter()
                            .find(|(n, _)| *n == k)
                            .ok_or(format!("no setting {k}"))?
                            .1,
                    ),
                    None => print(&Value::Object(all.into_iter().collect())),
                }
                Ok(())
            }
            SettingsCmd::List => {
                for (k, v) in settings(c).await? {
                    println!("{k} = {v}");
                }
                Ok(())
            }
            SettingsCmd::Export => {
                print(&Value::Object(settings(c).await?.into_iter().collect()));
                Ok(())
            }
            SettingsCmd::Set { key, value } => {
                c.call(
                    &lx("admin.setSetting"),
                    &[],
                    Some(json!({"key": key, "value": setting_value(&value)})),
                )
                .await?;
                println!("Set {key}. It applies now.");
                Ok(())
            }
            SettingsCmd::Import { file, yes } => {
                let text = std::fs::read_to_string(&file)
                    .map_err(|e| format!("{}: {e}", file.display()))?;
                let map: serde_json::Map<String, Value> =
                    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
                confirm(&format!("This sets {} settings", map.len()), yes)?;
                for (k, v) in map {
                    match c
                        .call(
                            &lx("admin.setSetting"),
                            &[],
                            Some(json!({"key": k, "value": v})),
                        )
                        .await
                    {
                        Ok(_) => println!("set {k}"),
                        Err(e) => println!("skipped {k}: {e}"),
                    }
                }
                Ok(())
            }
        },
        AdminCmd::Operator(o) => match o {
            OperatorCmd::Add { username, role } => {
                let pw = secret(&format!("Password for {username} (12 characters or more)"))?;
                c.call(
                    &lx("owner.createOperator"),
                    &[],
                    Some(json!({"username": username, "password": pw, "role": role})),
                )
                .await?;
                println!("Added {username} ({role}).");
                Ok(())
            }
            OperatorCmd::Remove { username, yes } => {
                confirm(&format!("This disables the operator {username}"), yes)?;
                c.call(
                    &lx("owner.updateOperator"),
                    &[],
                    Some(json!({"username": username, "disabled": true})),
                )
                .await?;
                println!("Disabled {username}.");
                Ok(())
            }
            OperatorCmd::List => {
                print(&c.call(&lx("owner.listOperators"), &[], None).await?);
                Ok(())
            }
            OperatorCmd::ResetPassword { username } => {
                let pw = secret(&format!("New password for {username}"))?;
                c.call(
                    &lx("owner.updateOperator"),
                    &[],
                    Some(json!({"username": username, "password": pw})),
                )
                .await?;
                println!("Changed; {username}'s console sign-ins ended.");
                Ok(())
            }
            OperatorCmd::SetRole { username, role } => {
                c.call(
                    &lx("owner.updateOperator"),
                    &[],
                    Some(json!({"username": username, "role": role})),
                )
                .await?;
                println!("{username} is now {role}.");
                Ok(())
            }
        },
        AdminCmd::Moderation(m) => match m {
            ModerationCmd::Status { subject: s, blob } => {
                print(
                    &c.call(
                        "com.atproto.admin.getSubjectStatus",
                        &subject_query(&s, blob.as_deref()),
                        None,
                    )
                    .await?,
                );
                Ok(())
            }
            ModerationCmd::Takedown {
                subject: s,
                blob,
                reference,
            } => {
                let mut t = json!({"applied": true});
                if let Some(r) = reference {
                    t["ref"] = json!(r);
                }
                print(
                    &c.call(
                        "com.atproto.admin.updateSubjectStatus",
                        &[],
                        Some(json!({"subject": subject(&s, blob.as_deref()), "takedown": t})),
                    )
                    .await?,
                );
                Ok(())
            }
            ModerationCmd::Restore { subject: s, blob } => {
                print(&c.call("com.atproto.admin.updateSubjectStatus", &[], Some(json!({"subject": subject(&s, blob.as_deref()), "takedown": {"applied": false}}))).await?);
                Ok(())
            }
        },
        AdminCmd::Crawl(CrawlCmd::Request) => {
            print(
                &c.call(&lx("admin.requestCrawl"), &[], Some(json!({})))
                    .await?,
            );
            Ok(())
        }
        AdminCmd::Jobs(JobsCmd::List) => {
            print(&c.call(&lx("viewer.listJobs"), &[], None).await?);
            Ok(())
        }
        AdminCmd::Kryphocron(k) => {
            let v = match k {
                KryphocronCmd::RotateNow => {
                    c.call(&lx("admin.kryphocron.rotateNow"), &[], Some(json!({})))
                        .await?
                }
                KryphocronCmd::Status => {
                    c.call(&lx("viewer.kryphocron.getStatus"), &[], None)
                        .await?
                }
                KryphocronCmd::Audit { limit } => {
                    c.call(
                        &lx("viewer.kryphocron.getAuditLog"),
                        &q(&[("limit", &limit.to_string())]),
                        None,
                    )
                    .await?
                }
            };
            print(&v);
            Ok(())
        }
        AdminCmd::Call { nsid, params, body } => {
            let query: Vec<(String, String)> = params
                .iter()
                .map(|p| {
                    p.split_once('=')
                        .map(|(k, v)| (k.to_owned(), v.to_owned()))
                        .ok_or(format!("{p}: expected key=value"))
                })
                .collect::<Result<_, _>>()?;
            let body = body
                .map(|b| serde_json::from_str::<Value>(&b).map_err(|e| format!("--body: {e}")))
                .transpose()?;
            print(&c.call(&nsid, &query, body).await?);
            Ok(())
        }
    }
}

async fn account(c: &Client, a: AccountCmd) -> Result<(), String> {
    let lx = |m: &str| format!("{NS}.{m}");
    let repo = |did: &str| json!({"$type": "com.atproto.admin.defs#repoRef", "did": did});
    match a {
        AccountCmd::List { query } => {
            let v = c
                .call(
                    "com.atproto.admin.searchAccounts",
                    &q(&[("q", query.as_deref().unwrap_or(""))]),
                    None,
                )
                .await?;
            for acct in v["accounts"].as_array().cloned().unwrap_or_default() {
                println!(
                    "{}  @{}  {}",
                    acct["did"].as_str().unwrap_or(""),
                    acct["handle"].as_str().unwrap_or(""),
                    acct["email"].as_str().unwrap_or("")
                );
            }
        }
        AccountCmd::Show { did } => {
            let mut info = c
                .call(
                    "com.atproto.admin.getAccountInfo",
                    &q(&[("did", &did)]),
                    None,
                )
                .await?;
            info["subjectStatus"] = c
                .call(
                    "com.atproto.admin.getSubjectStatus",
                    &q(&[("did", &did)]),
                    None,
                )
                .await?;
            print(&info);
        }
        AccountCmd::Create { handle, email } => {
            let pw = secret(&format!("Password for {handle}"))?;
            let mut body = json!({"handle": handle, "email": email, "password": pw});
            let invite = c
                .call(
                    "com.atproto.server.createInviteCode",
                    &[],
                    Some(json!({"useCount": 1})),
                )
                .await?;
            body["inviteCode"] = invite["code"].clone();
            let v = c
                .call("com.atproto.server.createAccount", &[], Some(body))
                .await?;
            println!(
                "Created @{} ({}).",
                v["handle"].as_str().unwrap_or(""),
                v["did"].as_str().unwrap_or("")
            );
        }
        AccountCmd::Delete { did, yes } => {
            confirm(
                &format!("This deletes {did}: its repo, records and blobs, for good"),
                yes,
            )?;
            c.call(
                "com.atproto.admin.deleteAccount",
                &[],
                Some(json!({"did": did})),
            )
            .await?;
            println!("Deleting {did}.");
        }
        AccountCmd::Takedown { did, reference } => {
            let mut t = json!({"applied": true});
            if let Some(r) = reference {
                t["ref"] = json!(r);
            }
            c.call(
                "com.atproto.admin.updateSubjectStatus",
                &[],
                Some(json!({"subject": repo(&did), "takedown": t})),
            )
            .await?;
            println!("Taken down.");
        }
        AccountCmd::Untakedown { did } => {
            c.call(
                "com.atproto.admin.updateSubjectStatus",
                &[],
                Some(json!({"subject": repo(&did), "takedown": {"applied": false}})),
            )
            .await?;
            println!("Takedown reversed.");
        }
        AccountCmd::Suspend { did } => {
            c.call(
                "com.atproto.admin.updateSubjectStatus",
                &[],
                Some(json!({"subject": repo(&did), "deactivated": {"applied": true}})),
            )
            .await?;
            println!("Deactivated.");
        }
        AccountCmd::Unsuspend { did } => {
            c.call(
                "com.atproto.admin.updateSubjectStatus",
                &[],
                Some(json!({"subject": repo(&did), "deactivated": {"applied": false}})),
            )
            .await?;
            println!("Reactivated.");
        }
        AccountCmd::ResetPassword { did } => {
            let pw = secret("New password")?;
            c.call(
                "com.atproto.admin.updateAccountPassword",
                &[],
                Some(json!({"did": did, "password": pw})),
            )
            .await?;
            println!("Changed; every sign-in of the account ended.");
        }
        AccountCmd::SetHandle { did, handle } => {
            c.call(
                "com.atproto.admin.updateAccountHandle",
                &[],
                Some(json!({"did": did, "handle": handle})),
            )
            .await?;
            println!("Handle changed.");
        }
        AccountCmd::SetEmail { did, email } => {
            c.call(
                "com.atproto.admin.updateAccountEmail",
                &[],
                Some(json!({"account": did, "email": email})),
            )
            .await?;
            println!("Email changed.");
        }
        AccountCmd::PrivateTier { did, state } => {
            let blocked = matches!(state, BlockState::Block);
            c.call(
                &lx("moderator.account.setPrivateTierBlock"),
                &[],
                Some(json!({"did": did, "blocked": blocked})),
            )
            .await?;
            println!(
                "{}",
                if blocked {
                    "Private posts blocked here."
                } else {
                    "Private posts allowed again."
                }
            );
        }
        AccountCmd::ImportStatus { did } => {
            print(
                &c.call(
                    &lx("viewer.account.getImportStatus"),
                    &q(&[("did", &did)]),
                    None,
                )
                .await?,
            );
        }
    }
    Ok(())
}

/// `healthcheck`: GET `/xrpc/_health`, exit 0 on 200.
pub async fn healthcheck(url: &str) -> i32 {
    let target = format!("{}/xrpc/_health", url.trim_end_matches('/'));
    match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => match c.get(&target).send().await {
            Ok(r) if r.status().is_success() => 0,
            Ok(r) => {
                eprintln!("{target}: HTTP {}", r.status());
                1
            }
            Err(e) => {
                eprintln!("{target}: {e}");
                1
            }
        },
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunked_bodies_decode() {
        assert_eq!(
            dechunk(b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"),
            b"hello world"
        );
        assert_eq!(dechunk(b""), b"");
    }

    #[test]
    fn setting_values_are_json_or_text() {
        assert_eq!(setting_value("true"), json!(true));
        assert_eq!(setting_value("[\"a\"]"), json!(["a"]));
        assert_eq!(setting_value("info"), json!("info"));
    }
}
