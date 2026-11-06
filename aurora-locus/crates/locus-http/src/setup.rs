//! First-run setup: setup mode's gate, the wizard, the final checks, and
//! the one transaction that applies a plan (shared with the CLI and
//! headless setup, so every path ends in the same state).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use locus_core::keys::MasterKey;
use locus_core::settings::Snapshot;
use locus_core::setup::{self as plan_rules, CLOUDFLARE_BODY_CAP, EmailPlan, ProxyMode, SetupPlan};
use locus_core::{CoreError, CoreResult};
use locus_store::Store;
use locus_store::email::{Sender, SenderStatus};
use locus_web::setup::{
    CheckRow, Choice, DnsRow, SetupForm, Step, StepPage, TokenPage, step_links,
};
use locus_web::view::{Link, MessagePage, Shell};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::state::{AppState, secret_names};
use crate::web;

type St = State<Arc<AppState>>;

const SESSION_COOKIE: &str = "locus-setup";
const SESSION_TTL_MS: i64 = 12 * 3_600_000;
const TEST_CODE_TTL_MS: i64 = 15 * 60_000;
/// Who the audit log names for setup's changes.
pub const ACTOR: &str = "setup";

// ---------------------------------------------------------------- the gate

/// What a node in setup mode knows about itself, for the wizard's pages.
#[derive(Debug, Clone, Default)]
pub struct SetupInfo {
    /// "SQLite" or "Postgres".
    pub database: String,
    /// Where blobs are kept.
    pub blob_store: String,
    /// The data directory (the storage test writes there).
    pub data_dir: PathBuf,
    /// Where the setup token file is, if this node wrote one.
    pub token_path: String,
    pub master_key_path: String,
}

/// Whether this node is in setup mode. Setup mode lasts until setup
/// completes (here or on any node); then `completed` fires and the node
/// restarts into normal service.
#[derive(Debug)]
pub struct Gate {
    active: AtomicBool,
    pub bootstrap_hostname: Option<String>,
    pub dev: bool,
    pub info: SetupInfo,
    pub completed: CancellationToken,
}

impl Gate {
    /// A node whose install finished setup.
    #[must_use]
    pub fn finished() -> Self {
        Self {
            active: AtomicBool::new(false),
            bootstrap_hostname: None,
            dev: false,
            info: SetupInfo::default(),
            completed: CancellationToken::new(),
        }
    }

    /// A node in setup mode.
    #[must_use]
    pub fn setup_mode(bootstrap_hostname: Option<String>, dev: bool, info: SetupInfo) -> Self {
        Self {
            active: AtomicBool::new(true),
            bootstrap_hostname,
            dev,
            info,
            completed: CancellationToken::new(),
        }
    }

    #[must_use]
    pub fn active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// Setup completed: nothing more is set up here, and the node restarts.
    pub fn complete(&self) {
        self.completed.cancel();
    }
}

fn is_setup_path(path: &str) -> bool {
    path == "/setup" || path.starts_with("/setup/")
}

/// In setup mode only the wizard, its assets, health and `/tls-check`
/// answer; other XRPC gets 503. Outside it the wizard is gone.
pub async fn gate(State(s): St, req: Request<Body>, next: Next) -> Response {
    let path = req.uri().path().to_owned();
    if !s.setup.active() {
        if is_setup_path(&path) {
            return (StatusCode::NOT_FOUND, "Not found").into_response();
        }
        return next.run(req).await;
    }
    if is_setup_path(&path)
        || path.starts_with("/static/")
        || path == "/xrpc/_health"
        || path == "/healthz"
    {
        return next.run(req).await;
    }
    match path.as_str() {
        "/" => web::redirect("/setup"),
        "/readyz" => (StatusCode::SERVICE_UNAVAILABLE, "setting up").into_response(),
        "/tls-check" => tls_check(&s, req.uri().query().unwrap_or("")),
        p if p.starts_with("/xrpc/") => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "ServiceUnavailable",
                "message": "This PDS is being set up.",
            })),
        )
            .into_response(),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "This PDS is being set up.").into_response(),
    }
}

/// Setup mode's `/tls-check`: the proxy may get a certificate for the
/// bootstrap hostname only, so `https://<host>/setup` loads.
fn tls_check(s: &AppState, query: &str) -> Response {
    let domain = url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == "domain")
        .map(|(_, v)| plan_rules::normalize_host(&v));
    match (&s.setup.bootstrap_hostname, domain) {
        (Some(h), Some(d)) if *h == d => StatusCode::OK.into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Watches for setup completing on any node (every 2 s), then fires the
/// gate's `completed`.
pub async fn watch(s: Arc<AppState>, cancel: CancellationToken) {
    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            () = s.setup.completed.cancelled() => return,
            () = tokio::time::sleep(Duration::from_secs(2)) => {}
        }
        match s.store.setup_completed().await {
            Ok(true) => {
                tracing::info!("setup completed; restarting into service");
                s.setup.complete();
                return;
            }
            Ok(false) => {}
            Err(e) => tracing::warn!("setup state check failed: {e}"),
        }
    }
}

// ---------------------------------------------------------------- the token

/// A new setup token: 32 characters, shown once.
#[must_use]
pub fn new_token() -> String {
    hex::encode(rand::random::<[u8; 16]>())
}

#[must_use]
pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(
        format!("locus-setup-token:{}", token.trim()).as_bytes(),
    ))
}

// ---------------------------------------------------------------- the plan

/// A plan seeded from what's already known: the bootstrap hostname and the
/// current settings (env values are applied before setup), with the
/// installer's defaults for the network services.
#[must_use]
pub fn seed_plan(snap: &Snapshot, bootstrap_hostname: Option<&str>) -> SetupPlan {
    let text = |k: &str| snap.text(k).to_owned();
    let or = |v: String, d: &str| if v.is_empty() { d.to_owned() } else { v };
    let mut services = plan_rules::ServicesPlan::default();
    let (appview_url, appview_did) = (text("appview.url"), text("appview.did"));
    if !appview_url.is_empty() {
        services.appview_url = appview_url;
        services.appview_did = appview_did;
    }
    let crawlers: Vec<String> = snap
        .list("sync.crawlers")
        .into_iter()
        .map(str::to_owned)
        .collect();
    if !crawlers.is_empty() {
        services.crawlers = crawlers;
    }
    services.moderation_url = text("moderation.service_url");
    services.moderation_did = text("moderation.service_did");
    services.report_url = or(text("moderation.report_service_url"), &services.report_url);
    services.report_did = or(text("moderation.report_service_did"), &services.report_did);
    let mut plan = SetupPlan {
        hostname: bootstrap_hostname.unwrap_or("").to_owned(),
        service_name: text("server.service_name"),
        contact_email: text("server.contact_email"),
        proxy: ProxyMode::parse(snap.text("deploy.proxy_mode")).unwrap_or_default(),
        handle_domains: snap
            .list("identity.handle_domains")
            .into_iter()
            .map(str::to_owned)
            .collect(),
        email: EmailPlan {
            from_address: text("email.from_address"),
            ..EmailPlan::default()
        },
        services,
        private_tier: snap.bool("kryphocron.private_tier_enabled"),
        recovery_did_key: text("identity.recovery_did_key"),
        ..SetupPlan::default()
    };
    plan.normalize();
    plan
}

/// Why a plan wasn't applied.
#[derive(Debug)]
pub enum ApplyError {
    Invalid(Vec<String>),
    /// Another node or run completed setup first; nothing was written.
    AlreadyComplete,
    Store(CoreError),
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(p) => write!(f, "{}", p.join(" ")),
            Self::AlreadyComplete => write!(f, "setup was already completed"),
            Self::Store(e) => write!(f, "{e}"),
        }
    }
}

impl From<CoreError> for ApplyError {
    fn from(e: CoreError) -> Self {
        Self::Store(e)
    }
}

/// Applies a plan and completes setup, in one transaction: the settings,
/// the first operator, the secrets it carries, the audit entry and the
/// completion mark all commit together or not at all, so two nodes
/// finishing at once can't both apply.
pub async fn apply(
    store: &Store,
    key: &MasterKey,
    plan: &SetupPlan,
    dev: bool,
) -> Result<(), ApplyError> {
    let problems = plan.problems(dev);
    if !problems.is_empty() {
        return Err(ApplyError::Invalid(
            problems.into_iter().map(|p| p.message).collect(),
        ));
    }
    let password_hash = match (&plan.operator.password, &plan.operator.password_hash) {
        _ if !plan.has_operator() => None,
        (Some(p), _) => Some(locus_auth::password::hash(p).map_err(CoreError::Invalid)?),
        (None, Some(h)) => Some(h.clone()),
        (None, None) => unreachable!("problems() requires one"),
    };
    let s = &plan.services;
    let mut settings: Vec<(&str, Value)> = vec![
        ("identity.handle_domains", json!(plan.handle_domains)),
        ("identity.reserved_handles", json!(plan.reserved_handles())),
        ("deploy.proxy_mode", json!(plan.proxy.as_str())),
        ("appview.url", json!(s.appview_url)),
        ("appview.did", json!(s.appview_did)),
        ("sync.crawlers", json!(s.crawlers)),
        ("moderation.service_url", json!(s.moderation_url)),
        ("moderation.service_did", json!(s.moderation_did)),
        ("moderation.report_service_url", json!(s.report_url)),
        ("moderation.report_service_did", json!(s.report_did)),
        ("kryphocron.private_tier_enabled", json!(plan.private_tier)),
        ("identity.recovery_did_key", json!(plan.recovery_did_key)),
        ("server.service_name", json!(plan.service_name)),
        ("server.contact_email", json!(plan.contact_email)),
    ];
    if !plan.trusted_proxies.is_empty() {
        settings.push(("http.trusted_proxies", json!(plan.trusted_proxies)));
    }
    if !plan.email.skipped {
        settings.push(("email.from_address", json!(plan.email.from_address)));
    }

    // Offline (no node has started yet) there's no setup row to complete:
    // make one, as a node would. A row already there stays as it is.
    store.create_setup_token(&token_hash(&new_token())).await?;
    let mut tx = store.begin().await?;
    // The completion mark first: it's the conditional write that decides
    // which of two runs applies, and the loser writes nothing else.
    if !tx.complete_setup(&plan.hostname, ACTOR).await? {
        tx.rollback().await?;
        return Err(ApplyError::AlreadyComplete);
    }
    for (k, v) in settings {
        tx.set_setting(k, v, ACTOR).await?;
    }
    if let Some(hash) = &password_hash
        && !tx
            .create_operator(&plan.operator.username, hash, "owner")
            .await?
    {
        tx.rollback().await?;
        return Err(ApplyError::Invalid(vec![format!(
            "An operator named {} already exists.",
            plan.operator.username
        )]));
    }
    if let Some(pw) = plan.admin_password.as_deref().filter(|p| !p.is_empty()) {
        tx.put_secret(secret_names::ADMIN_PASSWORD, pw.as_bytes(), key)
            .await?;
    }
    if let Some(url) = plan.email.smtp_url.as_deref().filter(|u| !u.is_empty())
        && !plan.email.skipped
    {
        tx.put_secret(
            locus_mail::smtp_secret_name(Sender::Main),
            url.as_bytes(),
            key,
        )
        .await?;
    }
    let detail = json!({
        "hostname": plan.hostname,
        "operator": plan.operator.username,
        "proxy": plan.proxy.as_str(),
        "handleDomains": plan.handle_domains,
        "email": !plan.email.skipped,
        "acknowledged": plan.acknowledged,
    });
    tx.audit(ACTOR, "setup.complete", Some(&plan.hostname), Some(&detail))
        .await?;
    tx.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------- checks

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Warn,
    Fail,
    Skip,
}

impl Outcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Skip => "skip",
        }
    }
}

/// One final check.
#[derive(Debug, Clone)]
pub struct Check {
    pub id: &'static str,
    pub label: &'static str,
    pub outcome: Outcome,
    pub detail: String,
}

fn check(
    id: &'static str,
    label: &'static str,
    outcome: Outcome,
    detail: impl Into<String>,
) -> Check {
    Check {
        id,
        label,
        outcome,
        detail: detail.into(),
    }
}

/// What the checks may look at.
pub struct CheckEnv<'a> {
    pub store: &'a Store,
    pub snap: &'a Snapshot,
    pub dev: bool,
    pub bootstrap_hostname: Option<&'a str>,
    /// The main sender's status, if it has an SMTP URL.
    pub email: Option<SenderStatus>,
    /// Probe the public URL (a node must be serving it in setup mode).
    pub probe: bool,
    /// A Cloudflare API token that can read the zone.
    pub cf_token: Option<&'a str>,
    pub cf_api: &'a str,
}

/// Cloudflare's API.
pub const CLOUDFLARE_API: &str = "https://api.cloudflare.com/client/v4";

/// Whether these results keep setup from finishing: any failure, or a
/// warning not accepted.
#[must_use]
pub fn blocking(checks: &[Check], acked: &[String]) -> bool {
    checks.iter().any(|c| {
        c.outcome == Outcome::Fail
            || (c.outcome == Outcome::Warn && !acked.iter().any(|a| a == c.id))
    })
}

/// The review step's checks, as pass/warn/fail. Only what would break the
/// PDS fails: an incomplete plan, or Cloudflare in Flexible mode. What may
/// just not be ready yet (DNS, reachability) warns.
pub async fn run_checks(env: &CheckEnv<'_>, plan: &SetupPlan) -> Vec<Check> {
    let mut out = Vec::new();
    let problems = plan.problems(env.dev);
    out.push(if problems.is_empty() {
        check("plan", "Answers", Outcome::Pass, "Every step is complete.")
    } else {
        check(
            "plan",
            "Answers",
            Outcome::Fail,
            problems
                .into_iter()
                .map(|p| p.message)
                .collect::<Vec<_>>()
                .join(" "),
        )
    });
    let host = plan.hostname.as_str();
    let local = host.is_empty() || locus_core::bootstrap::is_local_hostname(host);
    let cloudflare = plan.proxy == ProxyMode::Cloudflare;

    // DNS.
    if local {
        out.push(check(
            "dns",
            "DNS",
            Outcome::Skip,
            "Not needed for a local hostname.",
        ));
    } else {
        match tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host((host, 443))).await {
            Ok(Ok(addrs)) => {
                let mut ips: Vec<String> = addrs.map(|a| a.ip().to_string()).collect();
                ips.sort();
                ips.dedup();
                if ips.is_empty() {
                    out.push(check("dns", "DNS", Outcome::Warn, format!("{host} has no address yet.")));
                } else if cloudflare {
                    out.push(check("dns", "DNS", Outcome::Pass, format!(
                        "{host} resolves (to {}); behind Cloudflare these are Cloudflare's addresses, and the next check confirms they reach this server.",
                        ips.join(", ")
                    )));
                } else {
                    out.push(check("dns", "DNS", Outcome::Pass, format!("{host} resolves to {}.", ips.join(", "))));
                }
            }
            _ => out.push(check(
                "dns",
                "DNS",
                Outcome::Warn,
                format!("{host} doesn't resolve yet. Create the DNS records from the Handles step; changes can take a while to spread."),
            )),
        }
    }

    // Reachability through the public URL.
    if local || !env.probe {
        out.push(check(
            "reachable",
            "Reachable over HTTPS",
            Outcome::Skip,
            if local {
                "Not needed for a local hostname."
            } else {
                "Not checked from the command line while no node serves setup."
            },
        ));
    } else {
        out.push(probe(env, plan).await);
    }

    // The certificate: in setup mode /tls-check approves only the bootstrap
    // hostname.
    if !local && !cloudflare && env.bootstrap_hostname != Some(host) {
        out.push(check(
            "certificate",
            "Certificate",
            Outcome::Warn,
            format!(
                "Until setup finishes, the proxy may get a certificate only for {}. {host} gets its certificate on its first visit after setup.",
                env.bootstrap_hostname.unwrap_or("no name (none was given at install)")
            ),
        ));
    }

    if cloudflare {
        out.push(cloudflare_ssl(env, host).await);
        if plan
            .handle_domains
            .iter()
            .any(|d| d.trim_start_matches('.').split('.').count() >= 3)
        {
            out.push(check(
                "cf_handles",
                "Handle certificates",
                Outcome::Warn,
                "Handles two levels deep (like alice.pds.example.com) aren't covered by Cloudflare's free certificate; they need an Advanced Certificate.",
            ));
        }
        let upload = env.snap.int("blobs.upload_limit");
        let import = env.snap.int("limits.import_size");
        if upload > CLOUDFLARE_BODY_CAP || import > CLOUDFLARE_BODY_CAP {
            out.push(check(
                "cf_body",
                "Upload size",
                Outcome::Warn,
                format!(
                    "Cloudflare's Free and Pro plans refuse request bodies over 100 MB, but uploads may be up to {} MB and imports up to {} MB here; larger ones will fail at Cloudflare.",
                    upload / 1_048_576,
                    import / 1_048_576
                ),
            ));
        }
        out.push(check(
            "cf_cache",
            "Cloudflare caching",
            Outcome::Warn,
            if env.snap.bool("kryphocron.private_media_gated") {
                "Bypass Cloudflare's cache for /xrpc/*, /oauth/*, /admin/* and /blob/*. With the private media gate on this is required: a cached private image would reach people outside its audience."
            } else {
                "Bypass Cloudflare's cache for /xrpc/*, /oauth/*, /admin/* and /blob/* (Cloudflare doesn't cache these by default; check no rule does)."
            },
        ));
    }

    // Email.
    out.push(if plan.email.skipped {
        check(
            "email",
            "Email",
            Outcome::Warn,
            "No email: people can't confirm their address, reset a password by email or get sign-in codes.",
        )
    } else {
        match env.email {
            Some(SenderStatus::Verified) => check("email", "Email", Outcome::Pass, "A test email arrived."),
            Some(SenderStatus::Unverified) => check(
                "email",
                "Email",
                Outcome::Pass,
                "The SMTP server accepted the connection; delivery isn't confirmed yet (send a test email from the console).",
            ),
            Some(SenderStatus::Failed) => check(
                "email",
                "Email",
                Outcome::Fail,
                "The SMTP server refused the connection. Fix the SMTP URL, or continue without email.",
            ),
            None => check("email", "Email", Outcome::Fail, "Enter an SMTP URL, or continue without email."),
        }
    });

    if plan.private_tier && plan.services.appview_url.is_empty() {
        out.push(check(
            "private",
            "Private posts",
            Outcome::Warn,
            "Private posts are on, but no AppView is set, so they won't work until one is.",
        ));
    }
    out
}

/// Fetches `https://<host>/setup/probe`: it answers with the nonce only if
/// it reached this install, and records whether it arrived as plain HTTP.
async fn probe(env: &CheckEnv<'_>, plan: &SetupPlan) -> Check {
    const ID: &str = "reachable";
    const LABEL: &str = "Reachable over HTTPS";
    let cloudflare = plan.proxy == ProxyMode::Cloudflare;
    let nonce = hex::encode(rand::random::<[u8; 16]>());
    if let Err(e) = env.store.set_setup_probe(&nonce).await {
        return check(
            ID,
            LABEL,
            Outcome::Warn,
            format!("Couldn't start the check: {e}"),
        );
    }
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => return check(ID, LABEL, Outcome::Warn, e.to_string()),
    };
    let url = format!("https://{}/setup/probe", plan.hostname);
    match client.get(&url).send().await {
        Ok(res) => {
            let body = res.text().await.unwrap_or_default();
            if body.trim() != nonce {
                return check(
                    ID,
                    LABEL,
                    Outcome::Warn,
                    format!(
                        "{url} answered, but not from this server. Check the DNS records and the proxy."
                    ),
                );
            }
            let seen = env
                .store
                .setup_state()
                .await
                .ok()
                .flatten()
                .and_then(|r| r.probe_seen)
                .unwrap_or_default();
            if seen == "http" {
                if cloudflare {
                    return check(
                        ID,
                        LABEL,
                        Outcome::Fail,
                        "The request reached this server as plain HTTP: Cloudflare's SSL/TLS mode is Flexible. Set it to Full (strict).",
                    );
                }
                return check(
                    ID,
                    LABEL,
                    Outcome::Warn,
                    "The request reached this server as plain HTTP. Make sure the proxy forwards with X-Forwarded-Proto: https.",
                );
            }
            check(
                ID,
                LABEL,
                Outcome::Pass,
                format!("{url} reaches this server."),
            )
        }
        Err(e) if e.is_redirect() => check(
            ID,
            LABEL,
            if cloudflare {
                Outcome::Fail
            } else {
                Outcome::Warn
            },
            if cloudflare {
                "The request went round in redirects: Cloudflare's SSL/TLS mode is probably Flexible. Set it to Full (strict).".to_owned()
            } else {
                format!("{url} redirects in a loop. Check the proxy's HTTPS redirect.")
            },
        ),
        Err(e) => check(
            ID,
            LABEL,
            Outcome::Warn,
            format!("Couldn't reach {url}: {e}"),
        ),
    }
}

/// Cloudflare's SSL mode, read with a token, or a warning to confirm it.
async fn cloudflare_ssl(env: &CheckEnv<'_>, host: &str) -> Check {
    const ID: &str = "cf_ssl";
    const LABEL: &str = "Cloudflare SSL mode";
    let Some(token) = env.cf_token.filter(|t| !t.is_empty()) else {
        return check(
            ID,
            LABEL,
            Outcome::Warn,
            "Without an API token this can't be read: make sure SSL/TLS is set to Full (strict) for this zone. Full (not strict) accepts any certificate at the origin.",
        );
    };
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => return check(ID, LABEL, Outcome::Warn, e.to_string()),
    };
    let get = |url: String| {
        let client = client.clone();
        async move {
            let res = client
                .get(url)
                .bearer_auth(token)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            res.json::<Value>().await.map_err(|e| e.to_string())
        }
    };
    // The zone is the longest suffix of the hostname Cloudflare knows.
    let labels: Vec<&str> = host.split('.').collect();
    let mut zone_id = None;
    for i in 0..labels.len().saturating_sub(1) {
        let name = labels[i..].join(".");
        match get(format!("{}/zones?name={name}", env.cf_api)).await {
            Ok(v) => {
                if let Some(id) = v["result"][0]["id"].as_str() {
                    zone_id = Some(id.to_owned());
                    break;
                }
            }
            Err(e) => {
                return check(
                    ID,
                    LABEL,
                    Outcome::Warn,
                    format!("Cloudflare's API didn't answer: {e}"),
                );
            }
        }
    }
    let Some(zone) = zone_id else {
        return check(
            ID,
            LABEL,
            Outcome::Warn,
            format!("The token can't see a zone for {host}."),
        );
    };
    match get(format!("{}/zones/{zone}/settings/ssl", env.cf_api)).await {
        Ok(v) => match v["result"]["value"].as_str() {
            Some("strict") => check(ID, LABEL, Outcome::Pass, "Full (strict)."),
            Some(mode) => check(
                ID,
                LABEL,
                Outcome::Fail,
                format!("The zone's SSL/TLS mode is {mode}. Set it to Full (strict)."),
            ),
            None => check(
                ID,
                LABEL,
                Outcome::Warn,
                "Cloudflare didn't say the SSL mode (the token may lack zone settings read).",
            ),
        },
        Err(e) => check(
            ID,
            LABEL,
            Outcome::Warn,
            format!("Cloudflare's API didn't answer: {e}"),
        ),
    }
}

/// `GET /setup/probe`: the current probe nonce, recording how the request
/// arrived (behind a proxy, its forwarded scheme).
pub async fn probe_answer(State(s): St, headers: HeaderMap) -> Response {
    let Ok(Some(row)) = s.store.setup_state().await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(nonce) = row.probe_nonce else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("direct")
        .to_ascii_lowercase();
    let _ = s.store.record_setup_probe(&scheme).await;
    let mut res = nonce.into_response();
    res.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    res
}

// ---------------------------------------------------------------- headless

/// The plan headless setup builds from env, on top of the seeded plan:
/// `LOCUS_SETUP_OPERATOR_USERNAME`/`_PASSWORD` (or the reference's
/// `PDS_ADMIN_PASSWORD`), `PDS_EMAIL_SMTP_URL` with
/// `PDS_EMAIL_FROM_ADDRESS`, `LOCUS_SETUP_PROXY` and
/// `LOCUS_SETUP_KEYS_BACKUP_ACKNOWLEDGED`.
#[must_use]
pub fn plan_from_env(seed: SetupPlan, env: &HashMap<String, String>) -> SetupPlan {
    let get = |k: &str| {
        env.get(k)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let mut plan = seed;
    if let Some(h) = get("PDS_HOSTNAME") {
        plan.hostname = h;
    }
    if let Some(u) = get("LOCUS_SETUP_OPERATOR_USERNAME") {
        plan.operator.username = u;
        plan.operator.password = get("LOCUS_SETUP_OPERATOR_PASSWORD");
    }
    plan.admin_password = get("PDS_ADMIN_PASSWORD");
    if let Some(p) = get("LOCUS_SETUP_PROXY").and_then(|p| ProxyMode::parse(&p)) {
        plan.proxy = p;
    }
    if let Some(d) = get("LOCUS_SETUP_EXISTING_SUBDOMAINS") {
        plan.existing_subdomains = web::lines(&d);
    }
    // The SMTP URL is already in the key store (the mailer imports it at
    // boot); without one, headless setup goes without email.
    plan.email.skipped = get("PDS_EMAIL_SMTP_URL").is_none();
    if let Some(f) = get("PDS_EMAIL_FROM_ADDRESS") {
        plan.email.from_address = f;
    }
    plan.keys_backup_acknowledged = get("LOCUS_SETUP_KEYS_BACKUP_ACKNOWLEDGED")
        .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes"));
    plan.normalize();
    plan
}

/// Headless setup (`LOCUS_SETUP_AUTO=true`): builds the plan from env,
/// runs the checks, and applies it unless one fails. Warnings are logged
/// and accepted. A failure leaves the node in setup mode.
pub async fn headless(s: &AppState, env: &HashMap<String, String>) -> Result<(), String> {
    let plan = plan_from_env(
        seed_plan(&s.snapshot(), s.setup.bootstrap_hostname.as_deref()),
        env,
    );
    let checks = run_checks(
        &CheckEnv {
            store: &s.store,
            snap: &s.snapshot(),
            dev: s.setup.dev,
            bootstrap_hostname: s.setup.bootstrap_hostname.as_deref(),
            email: s.mailer.status(Sender::Main).await.ok().flatten(),
            probe: true,
            cf_token: env.get("LOCUS_SETUP_CLOUDFLARE_TOKEN").map(String::as_str),
            cf_api: CLOUDFLARE_API,
        },
        &plan,
    )
    .await;
    let mut failed = Vec::new();
    for c in &checks {
        match c.outcome {
            Outcome::Fail => {
                tracing::error!(check = c.id, "setup check failed: {}", c.detail);
                failed.push(c.detail.clone());
            }
            Outcome::Warn => tracing::warn!(check = c.id, "setup check warns: {}", c.detail),
            Outcome::Pass | Outcome::Skip => {
                tracing::info!(check = c.id, "setup check: {}", c.detail)
            }
        }
    }
    if !failed.is_empty() {
        return Err(failed.join(" "));
    }
    let mut plan = plan;
    plan.acknowledged = checks
        .iter()
        .filter(|c| c.outcome == Outcome::Warn)
        .map(|c| c.id.to_owned())
        .collect();
    apply(&s.store, &s.master_key, &plan, s.setup.dev)
        .await
        .map_err(|e| e.to_string())?;
    s.setup.complete();
    Ok(())
}

// ---------------------------------------------------------------- the wizard

/// The wizard's saved state: the plan so far, and a pending test email.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Draft {
    seeded: bool,
    plan: SetupPlan,
    test: Option<TestMail>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TestMail {
    to: String,
    code_hash: String,
    expires_at: i64,
}

async fn load_draft(s: &AppState) -> CoreResult<Draft> {
    let row = s.store.setup_state().await?;
    let mut d: Draft = row
        .and_then(|r| serde_json::from_str(&r.draft_json).ok())
        .unwrap_or_default();
    if !d.seeded {
        d.plan = seed_plan(&s.snapshot(), s.setup.bootstrap_hostname.as_deref());
        d.seeded = true;
    }
    Ok(d)
}

async fn save_draft(s: &AppState, d: &Draft) -> CoreResult<()> {
    let mut d = d.clone();
    // Never kept, whatever path filled them.
    d.plan.operator.password = None;
    d.plan.email.smtp_url = None;
    d.plan.admin_password = None;
    let json = serde_json::to_string(&d).map_err(|e| CoreError::Storage(e.to_string()))?;
    s.store.save_setup_draft(&json).await?;
    Ok(())
}

fn session_hash(id: &str) -> String {
    hex::encode(Sha256::digest(
        format!("locus-setup-session:{id}").as_bytes(),
    ))
}

fn csrf_for(id: &str) -> String {
    hex::encode(&Sha256::digest(format!("locus-setup-csrf:{id}").as_bytes())[..16])
}

/// The wizard session from the cookie, if live.
async fn session(s: &AppState, headers: &HeaderMap) -> Option<String> {
    let all = web::cookies(headers);
    let id = web::cookie(&all, SESSION_COOKIE)?.to_owned();
    if id.len() != 64 {
        return None;
    }
    s.store
        .setup_session_valid(&session_hash(&id))
        .await
        .ok()?
        .then_some(id)
}

fn brand(s: &AppState) -> String {
    let n = s.snapshot().text("server.service_name").to_owned();
    if n.is_empty() { "Locus".to_owned() } else { n }
}

fn token_page(s: &AppState, error: Option<String>, status: StatusCode) -> Response {
    web::render(
        &TokenPage {
            shell: Shell::signed_out("Set up", brand(s)),
            error,
            token_path: s.setup.info.token_path.clone(),
        },
        status,
    )
}

fn complete_page(s: &AppState) -> Response {
    web::render(
        &MessagePage {
            shell: Shell::signed_out("Setup is complete", brand(s)),
            message: "This PDS is set up. It's restarting into service now; the console opens in a moment.".into(),
            link: Some(Link {
                href: "/admin".into(),
                label: "Go to the console".into(),
            }),
        },
        StatusCode::OK,
    )
}

/// `GET /setup`: the token page, or the wizard's first step.
pub async fn index(State(s): St, headers: HeaderMap) -> Response {
    if s.setup.completed.is_cancelled() || s.store.setup_completed().await.unwrap_or(false) {
        return complete_page(&s);
    }
    if session(&s, &headers).await.is_some() {
        return web::redirect(&Step::Hostname.href());
    }
    token_page(&s, None, StatusCode::OK)
}

/// `POST /setup/token`.
pub async fn enter_token(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    if !web::same_site(&headers) {
        return token_page(
            &s,
            Some("Request refused (cross-site).".into()),
            StatusCode::FORBIDDEN,
        );
    }
    let form = web::parse_form(&body);
    let token = web::field(&form, "token");
    // A wrong token costs the same time as a right one, and a little more.
    match s.store.setup_token_matches(&token_hash(token)).await {
        Ok(true) => {}
        Ok(false) => {
            tokio::time::sleep(Duration::from_millis(500)).await;
            return token_page(
                &s,
                Some("That isn't the setup token.".into()),
                StatusCode::UNAUTHORIZED,
            );
        }
        Err(e) => return token_page(&s, Some(e.to_string()), StatusCode::INTERNAL_SERVER_ERROR),
    }
    let id = hex::encode(rand::random::<[u8; 32]>());
    if let Err(e) = s
        .store
        .add_setup_session(&session_hash(&id), SESSION_TTL_MS)
        .await
    {
        return token_page(&s, Some(e.to_string()), StatusCode::INTERNAL_SERVER_ERROR);
    }
    let secure = if s.public_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    web::with_cookie(
        web::redirect(&Step::Hostname.href()),
        &format!(
            "{SESSION_COOKIE}={id}; Path=/setup; Max-Age={}; HttpOnly; SameSite=Strict{secure}",
            SESSION_TTL_MS / 1000
        ),
    )
}

/// `GET /setup/{step}`.
pub async fn show_step(State(s): St, headers: HeaderMap, Path(slug): Path<String>) -> Response {
    let Some(step) = Step::parse(&slug) else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };
    if s.store.setup_completed().await.unwrap_or(false) {
        return complete_page(&s);
    }
    let Some(id) = session(&s, &headers).await else {
        return web::redirect("/setup");
    };
    let draft = match load_draft(&s).await {
        Ok(d) => d,
        Err(e) => return token_page(&s, Some(e.to_string()), StatusCode::INTERNAL_SERVER_ERROR),
    };
    let checks = if step == Step::Review {
        Some(review_checks(&s, &draft.plan, None).await)
    } else {
        None
    };
    step_page(
        &s,
        &id,
        step,
        &draft,
        Vec::new(),
        None,
        checks,
        StatusCode::OK,
    )
    .await
}

async fn review_checks(s: &AppState, plan: &SetupPlan, cf_token: Option<&str>) -> Vec<Check> {
    let snap = s.snapshot();
    run_checks(
        &CheckEnv {
            store: &s.store,
            snap: &snap,
            dev: s.setup.dev,
            bootstrap_hostname: s.setup.bootstrap_hostname.as_deref(),
            email: s.mailer.status(Sender::Main).await.ok().flatten(),
            probe: true,
            cf_token,
            cf_api: CLOUDFLARE_API,
        },
        plan,
    )
    .await
}

/// `POST /setup/{step}`: saves the step's answers, then goes on to the
/// next step (or shows what's wrong).
/// `POST /setup/keys/backup`: the key backup, as a download (the Keys
/// step's form; its passphrase is typed twice).
pub async fn key_backup(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    if s.store.setup_completed().await.unwrap_or(false) {
        return complete_page(&s);
    }
    let Some(id) = session(&s, &headers).await else {
        return web::redirect("/setup");
    };
    let form = web::parse_form(&body);
    if !web::same_site(&headers) || !web::ct_eq(web::field(&form, "csrf"), &csrf_for(&id)) {
        return (
            StatusCode::FORBIDDEN,
            "Request refused: the form expired. Go back and try again.",
        )
            .into_response();
    }
    let pass = web::field(&form, "passphrase");
    if pass != web::field(&form, "passphrase2") {
        return (
            StatusCode::BAD_REQUEST,
            "The two passphrases differ. Go back and try again.",
        )
            .into_response();
    }
    match crate::keys::backup_stream(Arc::clone(&s), pass, "setup".into()).await {
        Ok(body) => crate::keys::backup_response(&s, body),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            format!("{} Go back and try again.", e.message),
        )
            .into_response(),
    }
}

pub async fn save_step(
    State(s): St,
    headers: HeaderMap,
    Path(slug): Path<String>,
    body: Bytes,
) -> Response {
    let Some(step) = Step::parse(&slug) else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };
    if s.store.setup_completed().await.unwrap_or(false) {
        return complete_page(&s);
    }
    let Some(id) = session(&s, &headers).await else {
        return web::redirect("/setup");
    };
    let form = web::parse_form(&body);
    if !web::same_site(&headers) || !web::ct_eq(web::field(&form, "csrf"), &csrf_for(&id)) {
        return (
            StatusCode::FORBIDDEN,
            "Request refused: the form expired. Go back and try again.",
        )
            .into_response();
    }
    let mut draft = match load_draft(&s).await {
        Ok(d) => d,
        Err(e) => return token_page(&s, Some(e.to_string()), StatusCode::INTERNAL_SERVER_ERROR),
    };
    let f = |k: &str| web::field(&form, k).to_owned();
    let mut errors: Vec<String> = Vec::new();
    let mut notice = None;
    let mut stay = false;
    let plan = &mut draft.plan;
    match step {
        Step::Hostname => {
            plan.hostname = plan_rules::normalize_host(&f("hostname"));
            plan.service_name = f("service_name");
            plan.contact_email = f("contact_email");
            // A new hostname suggests new handle domains.
            plan.handle_domains
                .retain(|d| plan.hostname.ends_with(d.as_str()));
            plan.normalize();
            errors = plan
                .hostname_problems(s.setup.dev)
                .into_iter()
                .map(|p| p.message)
                .collect();
        }
        Step::Storage => {}
        Step::Proxy => {
            plan.proxy = ProxyMode::parse(&f("proxy")).unwrap_or_default();
            plan.trusted_proxies = web::lines(&f("trusted_proxies"));
            errors = plan
                .hostname_problems(true)
                .into_iter()
                .filter(|p| p.field == "trusted_proxies")
                .map(|p| p.message)
                .collect();
        }
        Step::Handles => {
            let mut domains: Vec<String> = web::fields(&form, "domain")
                .into_iter()
                .map(str::to_owned)
                .collect();
            domains.extend(web::lines(&f("other_handle_domains")));
            plan.handle_domains = domains;
            plan.existing_subdomains = web::lines(&f("existing_subdomains"));
            let none = plan.handle_domains.is_empty();
            plan.normalize();
            if none {
                plan.handle_domains.clear();
            }
            errors = plan
                .handle_problems()
                .into_iter()
                .map(|p| p.message)
                .collect();
        }
        Step::Operator => {
            plan.operator.username = f("username");
            let (p1, p2) = (f("password"), f("password2"));
            if !p1.is_empty() || plan.operator.password_hash.is_none() {
                if p1 != p2 {
                    errors.push("The two passwords differ.".into());
                } else {
                    plan.operator.password = Some(p1);
                }
            }
            if errors.is_empty() {
                errors = plan
                    .operator_problems()
                    .into_iter()
                    .map(|p| p.message)
                    .collect();
            }
            if errors.is_empty()
                && let Some(p) = plan.operator.password.take()
            {
                match locus_auth::password::hash(&p) {
                    Ok(h) => plan.operator.password_hash = Some(h),
                    Err(e) => errors.push(e),
                }
            }
            plan.operator.password = None;
        }
        Step::Email => {
            stay = true;
            match f("do").as_str() {
                "save" => {
                    plan.email.from_address = f("from_address");
                    plan.email.skipped = false;
                    let url = f("smtp_url");
                    if !url.is_empty() {
                        plan.email.smtp_url = Some(url.clone());
                    }
                    errors = plan
                        .email_problems()
                        .into_iter()
                        .map(|p| p.message)
                        .collect();
                    plan.email.smtp_url = None;
                    if errors.is_empty() && !url.is_empty() {
                        match s.mailer.set_url(Sender::Main, &url).await {
                            Ok(SenderStatus::Failed) => errors.push(
                                "Saved, but the SMTP server refused the connection. Check the URL and try again.".into(),
                            ),
                            Ok(_) => notice = Some("Saved. The SMTP server accepted the connection; now send a test email.".into()),
                            Err(e) => errors.push(e.to_string()),
                        }
                    } else if errors.is_empty() {
                        notice = Some("Saved.".into());
                    }
                }
                "send" => {
                    let to = f("to");
                    let code = test_code();
                    let email = locus_mail::Template::TestEmail
                        .render(&[("service", &brand(&s)), ("token", &code)]);
                    match s
                        .mailer
                        .send(Sender::Main, &plan.email.from_address, &to, email)
                        .await
                    {
                        Ok(()) => {
                            draft.test = Some(TestMail {
                                to: to.clone(),
                                code_hash: test_code_hash(&code),
                                expires_at: locus_core::time::now_ms() + TEST_CODE_TTL_MS,
                            });
                            notice = Some(format!("Sent to {to}. Enter the code from it below."));
                        }
                        Err(e) => errors.push(format!("Sending failed: {e}")),
                    }
                }
                "verify" => {
                    let code = f("code").to_ascii_uppercase();
                    let ok = draft.test.as_ref().is_some_and(|t| {
                        t.expires_at > locus_core::time::now_ms()
                            && web::ct_eq(&test_code_hash(&code), &t.code_hash)
                    });
                    if ok {
                        match s.mailer.mark_verified(Sender::Main).await {
                            Ok(()) => {
                                draft.test = None;
                                notice = Some("The test email arrived: email works.".into());
                            }
                            Err(e) => errors.push(e.to_string()),
                        }
                    } else {
                        errors
                            .push("That code doesn't match (or it expired; send another).".into());
                    }
                }
                "skip" => {
                    plan.email.skipped = true;
                    stay = false;
                }
                _ => {
                    if email_ready(&s).await {
                        plan.email.skipped = false;
                        stay = false;
                    } else {
                        errors.push("Email isn't working yet: save an SMTP URL the server accepts, or continue without email.".into());
                    }
                }
            }
        }
        Step::Services => {
            let svc = &mut plan.services;
            svc.appview_url = f("appview_url");
            svc.appview_did = f("appview_did");
            svc.crawlers = web::lines(&f("crawlers"));
            svc.moderation_url = f("moderation_url");
            svc.moderation_did = f("moderation_did");
            svc.report_url = f("report_url");
            svc.report_did = f("report_did");
            errors = plan
                .services_problems()
                .into_iter()
                .map(|p| p.message)
                .collect();
        }
        Step::Private => {
            plan.private_tier = f("private_tier") == "1";
        }
        Step::Keys => {
            plan.recovery_did_key = f("recovery_did_key");
            plan.keys_backup_acknowledged = f("keys_ack") == "1";
            if !plan.recovery_did_key.is_empty() && !plan.recovery_did_key.starts_with("did:key:") {
                errors.push("A recovery key is a did:key.".into());
            }
            if !plan.keys_backup_acknowledged {
                errors.push("Confirm you have a key backup (or a copy of the master key): without one, every stored key is lost.".into());
            }
        }
        Step::Review => {
            plan.acknowledged = web::fields(&form, "ack")
                .into_iter()
                .map(str::to_owned)
                .collect();
            let cf_token = f("cf_token");
            let checks = review_checks(&s, &draft.plan, Some(&cf_token)).await;
            if let Err(e) = save_draft(&s, &draft).await {
                return token_page(&s, Some(e.to_string()), StatusCode::INTERNAL_SERVER_ERROR);
            }
            if f("do") != "finish" {
                return step_page(
                    &s,
                    &id,
                    step,
                    &draft,
                    Vec::new(),
                    None,
                    Some(checks),
                    StatusCode::OK,
                )
                .await;
            }
            if blocking(&checks, &draft.plan.acknowledged) {
                let e = vec![
                    "Fix the failed checks and accept each warning before finishing.".to_owned(),
                ];
                return step_page(
                    &s,
                    &id,
                    step,
                    &draft,
                    e,
                    None,
                    Some(checks),
                    StatusCode::BAD_REQUEST,
                )
                .await;
            }
            return match apply(&s.store, &s.master_key, &draft.plan, s.setup.dev).await {
                Ok(()) => {
                    tracing::info!(hostname = %draft.plan.hostname, "setup completed in the wizard");
                    s.setup.complete();
                    complete_page(&s)
                }
                Err(ApplyError::AlreadyComplete) => complete_page(&s),
                Err(e) => {
                    let e = vec![e.to_string()];
                    step_page(
                        &s,
                        &id,
                        step,
                        &draft,
                        e,
                        None,
                        Some(checks),
                        StatusCode::BAD_REQUEST,
                    )
                    .await
                }
            };
        }
    }
    if let Err(e) = save_draft(&s, &draft).await {
        errors.push(e.to_string());
    }
    if errors.is_empty() && !stay {
        return web::redirect(&step.next().unwrap_or(Step::Review).href());
    }
    let status = if errors.is_empty() {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    step_page(&s, &id, step, &draft, errors, notice, None, status).await
}

async fn email_ready(s: &AppState) -> bool {
    s.mailer
        .status(Sender::Main)
        .await
        .ok()
        .flatten()
        .is_some_and(SenderStatus::configured)
}

/// A test-email code like `ABCDE-FGH23`, without look-alike characters.
fn test_code() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let bytes: [u8; 10] = rand::random();
    let chars: String = bytes
        .iter()
        .map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()] as char)
        .collect();
    format!("{}-{}", &chars[..5], &chars[5..])
}

fn test_code_hash(code: &str) -> String {
    hex::encode(Sha256::digest(
        format!("locus-setup-test:{}", code.trim()).as_bytes(),
    ))
}

#[allow(clippy::too_many_arguments)]
async fn step_page(
    s: &AppState,
    id: &str,
    step: Step,
    draft: &Draft,
    errors: Vec<String>,
    notice: Option<String>,
    checks: Option<Vec<Check>>,
    status: StatusCode,
) -> Response {
    let f = form_for(s, step, draft, checks).await;
    web::render(
        &StepPage {
            shell: Shell::signed_out(format!("Setup: {}", step.title()), brand(s)),
            csrf: csrf_for(id),
            step,
            steps: step_links(step),
            errors,
            notice,
            f,
        },
        status,
    )
}

async fn form_for(
    s: &AppState,
    step: Step,
    draft: &Draft,
    checks: Option<Vec<Check>>,
) -> SetupForm {
    let p = &draft.plan;
    let mut f = SetupForm {
        hostname: p.hostname.clone(),
        service_name: p.service_name.clone(),
        contact_email: p.contact_email.clone(),
        dev: s.setup.dev,
        cloudflare: p.proxy == ProxyMode::Cloudflare,
        ..SetupForm::default()
    };
    match step {
        Step::Hostname => {}
        Step::Storage => {
            f.database = s.setup.info.database.clone();
            f.blob_store = s.setup.info.blob_store.clone();
            let (ok, detail) = storage_test(s).await;
            f.storage_ok = ok;
            f.storage_detail = detail;
        }
        Step::Proxy => {
            f.proxy_options = ProxyMode::ALL
                .into_iter()
                .map(|m| Choice {
                    value: m.as_str().into(),
                    label: m.label().into(),
                    checked: m == p.proxy,
                })
                .collect();
            f.trusted_proxies = p.trusted_proxies.join("\n");
        }
        Step::Handles => {
            let suggested = plan_rules::suggested_handle_domains(&p.hostname);
            f.handle_options = suggested
                .iter()
                .enumerate()
                .map(|(i, d)| Choice {
                    value: d.clone(),
                    label: format!(
                        "*{d}{}",
                        if i == 0 && suggested.len() > 1 {
                            " (suggested)"
                        } else {
                            ""
                        }
                    ),
                    checked: p.handle_domains.contains(d),
                })
                .collect();
            f.other_handle_domains = p
                .handle_domains
                .iter()
                .filter(|d| !suggested.contains(d))
                .map(|d| d.trim_start_matches('.').to_owned())
                .collect::<Vec<_>>()
                .join("\n");
            f.existing_subdomains = p.existing_subdomains.join("\n");
            f.dns = p
                .dns_records()
                .into_iter()
                .map(|(name, points_to, note)| DnsRow {
                    name,
                    points_to,
                    note,
                })
                .collect();
        }
        Step::Operator => {
            f.username = p.operator.username.clone();
            f.password_set = p.operator.password_hash.is_some();
        }
        Step::Email => {
            let status = s.mailer.status(Sender::Main).await.ok().flatten();
            f.smtp_url_set = status.is_some();
            (f.email_status, f.email_class) = match status {
                Some(SenderStatus::Verified) => ("working: a test email arrived".into(), "ok"),
                Some(SenderStatus::Unverified) => (
                    "connected; send a test email to confirm delivery".into(),
                    "warn",
                ),
                Some(SenderStatus::Failed) => {
                    ("the SMTP server refused the connection".into(), "bad")
                }
                None => ("not set up".into(), "muted"),
            };
            f.email_ready = status.is_some_and(SenderStatus::configured);
            f.from_address = p.email.from_address.clone();
            f.email_skipped = p.email.skipped;
            f.test_sent_to = draft
                .test
                .as_ref()
                .map(|t| t.to.clone())
                .unwrap_or_default();
        }
        Step::Services => {
            let svc = &p.services;
            f.appview_url = svc.appview_url.clone();
            f.appview_did = svc.appview_did.clone();
            f.crawlers = svc.crawlers.join("\n");
            f.moderation_url = svc.moderation_url.clone();
            f.moderation_did = svc.moderation_did.clone();
            f.report_url = svc.report_url.clone();
            f.report_did = svc.report_did.clone();
        }
        Step::Private => {
            f.private_tier = p.private_tier;
            f.appview_set = !p.services.appview_url.is_empty();
        }
        Step::Keys => {
            f.rotation_key = s.accepted_plc_keys().into_iter().next().unwrap_or_default();
            f.master_key_source = match s.master_key.source().as_str() {
                "file" => "A key file".into(),
                "env" => "From the environment".into(),
                other => other.to_owned(),
            };
            f.master_key_id = s.master_key.key_id();
            f.master_key_path = s.setup.info.master_key_path.clone();
            f.recovery_did_key = p.recovery_did_key.clone();
            f.keys_ack = p.keys_backup_acknowledged;
        }
        Step::Review => {
            let checks = checks.unwrap_or_default();
            f.blocking = blocking(&checks, &p.acknowledged);
            f.checks = checks
                .into_iter()
                .map(|c| CheckRow {
                    acked: p.acknowledged.iter().any(|a| a == c.id),
                    id: c.id.into(),
                    label: c.label.into(),
                    outcome: c.outcome.as_str(),
                    detail: c.detail,
                })
                .collect();
            let or_none = |v: &str| {
                if v.is_empty() {
                    "none".to_owned()
                } else {
                    v.to_owned()
                }
            };
            f.summary = vec![
                ("Hostname".into(), or_none(&p.hostname)),
                ("Proxy".into(), p.proxy.label().into()),
                (
                    "Handles".into(),
                    or_none(
                        &p.handle_domains
                            .iter()
                            .map(|d| format!("*{d}"))
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                ),
                (
                    "Reserved handles".into(),
                    or_none(&p.reserved_handles().join(", ")),
                ),
                ("Owner".into(), or_none(&p.operator.username)),
                (
                    "Email".into(),
                    if p.email.skipped {
                        "none".into()
                    } else {
                        or_none(&p.email.from_address)
                    },
                ),
                ("AppView".into(), or_none(&p.services.appview_url)),
                ("Moderation".into(), or_none(&p.services.moderation_url)),
                ("Reports".into(), or_none(&p.services.report_url)),
                (
                    "Private posts".into(),
                    if p.private_tier {
                        "on".into()
                    } else {
                        "off".into()
                    },
                ),
            ];
        }
    }
    f
}

/// The storage step's test: a database round trip, and a file written to
/// and read back from the data directory.
async fn storage_test(s: &AppState) -> (bool, String) {
    if let Err(e) = s.store.now_ms().await {
        return (false, format!("the database didn't answer: {e}"));
    }
    let dir = &s.setup.info.data_dir;
    let path = dir.join(format!(
        ".setup-test-{}",
        hex::encode(rand::random::<[u8; 4]>())
    ));
    let result = async {
        tokio::fs::create_dir_all(dir).await?;
        tokio::fs::write(&path, b"locus").await?;
        let back = tokio::fs::read(&path).await?;
        tokio::fs::remove_file(&path).await?;
        Ok::<_, std::io::Error>(back == b"locus")
    }
    .await;
    match result {
        Ok(true) => (true, "the database answered, and a file was written to and read back from the data directory.".into()),
        Ok(false) => (false, "a file read back from the data directory differs from what was written".into()),
        Err(e) => (false, format!("the data directory {} isn't writable: {e}", dir.display())),
    }
}
