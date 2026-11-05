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
        let path = if query.is_empty() {
            format!("/xrpc/{nsid}")
        } else {
            format!("/xrpc/{nsid}?{}", query_string(query))
        };
        let (status, bytes) = match self {
            Self::Http { url, token } => {
                let client = reqwest::Client::new();
                let url = format!("{}{path}", url.trim_end_matches('/'));
                let req = match &body {
                    Some(b) => client.post(url).json(b),
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
                let payload = body.as_ref().map(Value::to_string).unwrap_or_default();
                let head = format!(
                    "{} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                    if body.is_some() { "POST" } else { "GET" },
                    payload.len()
                );
                stream
                    .write_all(head.as_bytes())
                    .await
                    .map_err(|e| e.to_string())?;
                stream
                    .write_all(payload.as_bytes())
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
        let v: Value = if bytes.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        if (200..300).contains(&status) {
            Ok(v)
        } else {
            Err(match (v["error"].as_str(), v["message"].as_str()) {
                (Some(e), Some(m)) => format!("{e}: {m}"),
                (_, Some(m)) => m.to_owned(),
                _ => format!("HTTP {status}: {v}"),
            })
        }
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

/// Runs an online command.
pub async fn run(c: &Client, cmd: AdminCmd) -> Result<(), String> {
    let lx = |m: &str| format!("{NS}.{m}");
    match cmd {
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
