//! The console's hostname-migration wizard and the reference-import report.
//! The wizard drives the admin methods `aurora-locus migrate hostname`
//! uses: prepare a candidate, its pre-checks and certificate, confirm (owner,
//! freshly confirmed, typing the new name), the job's progress and
//! stragglers, cancel, and close after the window.

#![allow(clippy::result_large_err)]

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use locus_web::console::PageId;
use locus_web::console_pages::{Card, Cell, Fact, Form, Input, SectionPage, Table};
use locus_web::view::{Banner, ConfirmPage, Hidden, Link};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::admin_auth::Role;
use crate::console::{Op, form_op, need_confirmed, require_op, shell, when};
use crate::console_people::account_href;
use crate::state::AppState;
use crate::web;

type St = State<Arc<AppState>>;

const CONFIRM_PATH: &str = "/admin/hostname/confirm";

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/hostname", get(page))
        .route("/admin/hostname/prepare", post(prepare))
        .route(CONFIRM_PATH, get(confirm_page).post(confirm))
        .route("/admin/hostname/cancel", post(cancel))
        .route("/admin/hostname/close", post(close))
        .route("/admin/import", get(import_report))
}

fn api(r: crate::error::XrpcResult<Json<Value>>) -> Result<Value, String> {
    r.map(|j| j.0).map_err(|e| e.message)
}

fn input<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).expect("the console builds valid API inputs")
}

fn cols(c: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    c.iter().map(|(k, l)| (*k, (*l).to_owned())).collect()
}

fn at(v: &Value) -> String {
    v.as_i64().map_or_else(|| "—".into(), when)
}

fn blank(form: &str, text: &str, action: &str, button: &str, danger: bool) -> Form {
    Form {
        title: form.into(),
        text: text.into(),
        action: action.into(),
        get: false,
        inputs: Vec::new(),
        hidden: Vec::new(),
        button: button.into(),
        danger,
    }
}

/// The hostname status, as `aurora-locus migrate hostname --status` gets
/// it; with `checks`, a prepared candidate's pre-checks are run fresh.
async fn status(s: &Arc<AppState>, op: &Op, checks: bool) -> Result<Value, String> {
    let uri: axum::http::Uri = format!("/?checks={checks}").parse().unwrap_or_default();
    let q = Query::try_from_uri(&uri).map_err(|e| e.to_string())?;
    api(crate::hostnames::status(State(Arc::clone(s)), op.api_headers(), q).await)
}

/// The prepared or running migration in a status.
fn open(v: &Value) -> Option<&Value> {
    v["migrations"]
        .as_array()?
        .iter()
        .find(|m| matches!(m["state"].as_str(), Some("prepared" | "running")))
}

#[derive(Deserialize, Default)]
pub struct HostQuery {
    #[serde(default)]
    done: Option<String>,
    /// Run a prepared candidate's pre-checks now.
    #[serde(default)]
    check: Option<String>,
}

/// `GET /admin/hostname?check=1`.
pub async fn page(State(s): St, headers: HeaderMap, Query(q): Query<HostQuery>) -> Response {
    let op = match require_op(&s, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let notice = q.done.map(|d| match d.as_str() {
        "prepared" => "Prepared. Its pre-checks ran below; confirm once they all pass.".to_owned(),
        "confirmed" => "Confirmed: the new hostname is current, and the job is moving every account's PDS endpoint.".to_owned(),
        "cancelled" => "Cancelled: the candidate is cleared.".to_owned(),
        "closed" => "Closed. Its stragglers stay listed, and the old host keeps serving them.".to_owned(),
        other => other.to_owned(),
    });
    let p = render(&s, &op, q.check.is_some(), notice, Vec::new()).await;
    web::render_shell(&p, &p.shell, StatusCode::OK)
}

async fn render(
    s: &Arc<AppState>,
    op: &Op,
    checks: bool,
    notice: Option<String>,
    errors: Vec<String>,
) -> SectionPage {
    let mut p = crate::console_sections::section(
        s,
        op,
        PageId::Hostname,
        "The hostname is this server's service DID (did:web:<host>), its OAuth issuer and its relays' name for it, so changing it is a migration, not a setting: prepare the new name, check it, confirm, and a job moves every account.",
    )
    .await;
    p.notice = notice;
    p.errors = errors;
    // Only a running migration's progress changes by itself.
    p.shell.live.clear();
    if s.role != locus_core::bootstrap::Role::Pds {
        p.cards.push(Card {
            title: "Not in this role".into(),
            facts: vec![
                Fact::new(
                    "Hostname migration",
                    "refused in the entryway and shard roles in this version",
                )
                .class("warn"),
            ],
        });
        return p;
    }
    let v = match status(s, op, checks).await {
        Ok(v) => v,
        Err(e) => {
            p.errors.push(e);
            return p;
        }
    };
    let list = |k: &str| {
        let l: Vec<String> = v[k]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|h| h.as_str().map(str::to_owned))
            .collect();
        if l.is_empty() {
            "none".to_owned()
        } else {
            l.join(", ")
        }
    };
    p.cards.push(Card {
        title: "This server".into(),
        facts: vec![
            Fact::new("Hostname", v["hostname"].as_str().unwrap_or_default()),
            Fact::new("Old hostnames still served", list("oldHostsServed")),
            Fact::new("Former hostnames", list("formerHosts")),
        ],
    });
    let owner = op.role >= Role::Owner;
    match open(&v) {
        None => {
            if owner {
                p.forms.push(Form {
                    title: "1. Prepare a new hostname".into(),
                    text: "Records the name as a candidate and changes nothing else. It is reserved against handles from now on, and /tls-check approves it so its certificate can be issued. Then point its DNS at this server.".into(),
                    action: "/admin/hostname/prepare".into(),
                    get: false,
                    inputs: vec![Input::new("host", "New hostname", "text").hint("pds.example.org")],
                    hidden: Vec::new(),
                    button: "Prepare".into(),
                    danger: false,
                });
            } else {
                p.intro.push_str(" Only an owner can start one.");
            }
        }
        Some(m) if m["state"] == "prepared" => prepared(&mut p, &v, m, owner),
        Some(m) => {
            running(&mut p, m, owner, s.store.db_now_ms().await.unwrap_or(0));
            p.shell.live = locus_web::console::live_src(PageId::Hostname);
        }
    }
    // Finished migrations whose old host some documents still name.
    for m in v["migrations"].as_array().into_iter().flatten() {
        if m["state"] == "done" && m["formerAt"].is_null() {
            stragglers(&mut p, m);
            p.shell.live = locus_web::console::live_src(PageId::Hostname);
        }
    }
    p.tables.push(Table {
        title: "Migrations".into(),
        cols: cols(&[
            ("name", "From"),
            ("name", "To"),
            ("state", "State"),
            ("when", "Confirmed"),
            ("when", "Closed"),
            ("when", "Old host former"),
        ]),
        rows: v["migrations"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|m| {
                let state = m["state"].as_str().unwrap_or_default().to_owned();
                let pill = match state.as_str() {
                    "running" | "prepared" => "amber",
                    _ => "fine",
                };
                vec![
                    Cell::new("name", m["from"].as_str().unwrap_or_default()),
                    Cell::new("name", m["to"].as_str().unwrap_or_default()),
                    Cell::new("state", state).pill(pill),
                    Cell::new("when", at(&m["confirmedAt"])),
                    Cell::new("when", at(&m["closedAt"])),
                    Cell::new("when", at(&m["formerAt"])),
                ]
            })
            .collect(),
        empty: "None yet.".into(),
    });
    p
}

fn prepared(p: &mut SectionPage, v: &Value, m: &Value, owner: bool) {
    let host = m["to"].as_str().unwrap_or_default();
    let checks: Vec<&Value> = v["prechecks"].as_array().into_iter().flatten().collect();
    let all_pass = !checks.is_empty() && checks.iter().all(|c| c["ok"] == json!(true));
    let find = |name: &str| checks.iter().find(|c| c["check"].as_str() == Some(name));
    let cert = find("resolves here and serves HTTPS with a valid certificate");
    let tls = find("/tls-check approves it");
    p.cards.push(Card {
        title: format!("2. Check {host}"),
        facts: vec![
            Fact::new("Candidate", host),
            Fact::new(
                "Prepared",
                format!(
                    "{} by {}",
                    at(&m["createdAt"]),
                    m["createdBy"].as_str().unwrap_or_default()
                ),
            ),
            match cert {
                Some(c) if c["ok"] == json!(true) => {
                    Fact::new("Certificate", "issued and valid").class("ok")
                }
                Some(c) => Fact::new(
                    "Certificate",
                    format!("not yet: {}", c["why"].as_str().unwrap_or_default()),
                )
                .class("warn"),
                None => Fact::new("Certificate", "not checked yet").class("warn"),
            },
            match tls {
                Some(c) if c["ok"] == json!(true) => {
                    Fact::new("/tls-check", "approves it").class("ok")
                }
                Some(_) => Fact::new("/tls-check", "doesn't approve it yet").class("warn"),
                None => Fact::new("/tls-check", "not checked yet").class("warn"),
            },
        ],
    });
    p.tables.push(Table {
        title: "Pre-checks".into(),
        cols: cols(&[("name", "Check"), ("state", "Result"), ("text", "Why")]),
        rows: checks
            .iter()
            .map(|c| {
                let ok = c["ok"] == json!(true);
                vec![
                    Cell::new("name", c["check"].as_str().unwrap_or_default()),
                    Cell::new("state", if ok { "passes" } else { "fails" }).pill(if ok {
                        "fine"
                    } else {
                        "red"
                    }),
                    Cell::new("text", c["why"].as_str().unwrap_or_default()),
                ]
            })
            .collect(),
        empty: "Not run yet: run them below.".into(),
    });
    p.forms.push(Form {
        title: "Run the pre-checks".into(),
        text: "Asks https://<new host>/xrpc/_health over TLS, which is also what gets its certificate issued; it can take a few seconds.".into(),
        action: "/admin/hostname".into(),
        get: true,
        inputs: Vec::new(),
        hidden: vec![("check".into(), "1".into())],
        button: "Check now".into(),
        danger: false,
    });
    if !owner {
        return;
    }
    if all_pass {
        p.forms.push(Form {
            title: "3. Confirm".into(),
            text: "Every OAuth session must sign in again; the next page says what else changes."
                .into(),
            action: CONFIRM_PATH.into(),
            get: true,
            inputs: Vec::new(),
            hidden: Vec::new(),
            button: "Confirm the migration…".into(),
            danger: true,
        });
    } else {
        p.intro
            .push_str(" Confirming opens once every pre-check passes.");
    }
    p.forms.push(blank(
        "Cancel",
        "Clears the candidate: /tls-check stops approving it. Its reservation stays only if it was a hostname here before.",
        "/admin/hostname/cancel",
        "Cancel the candidate",
        true,
    ));
}

fn running(p: &mut SectionPage, m: &Value, owner: bool, now: i64) {
    let progress = &m["progress"];
    let n = |k: &str| progress[k].as_i64().unwrap_or(0);
    let job_done = !m["jobDoneAt"].is_null();
    let window_over = m["windowEndsAt"].as_i64().is_some_and(|w| now >= w);
    p.cards.push(Card {
        title: format!(
            "Moving from {} to {}",
            m["from"].as_str().unwrap_or_default(),
            m["to"].as_str().unwrap_or_default()
        ),
        facts: vec![
            Fact::new("Confirmed", at(&m["confirmedAt"])),
            Fact::new(
                "The job",
                if job_done {
                    format!("done {}", at(&m["jobDoneAt"]))
                } else {
                    "moving accounts".into()
                },
            )
            .class(if job_done { "ok" } else { "warn" }),
            Fact::new("Accounts moved", n("moved").to_string()),
            Fact::new("Accounts it can't move", n("cannot").to_string())
                .class(if n("cannot") > 0 { "warn" } else { "" }),
            Fact::new("Moves that failed", n("failed").to_string()).class(if n("failed") > 0 {
                "bad"
            } else {
                ""
            }),
            Fact::new("Window ends", at(&m["windowEndsAt"])),
        ],
    });
    stragglers(p, m);
    if !owner {
        return;
    }
    if job_done && window_over {
        p.forms.push(blank(
            "Close the migration",
            "Ends it with its stragglers still listed, so another migration can start. The old host keeps full service while any DID document names it.",
            "/admin/hostname/close",
            "Close",
            true,
        ));
    } else {
        p.intro
            .push_str(" A migration can be closed once its job is done and its window has ended.");
    }
}

fn stragglers(p: &mut SectionPage, m: &Value) {
    let count = m["stragglerCount"].as_i64().unwrap_or(0);
    p.tables.push(Table {
        title: format!(
            "Accounts still pointing at {} ({count})",
            m["from"].as_str().unwrap_or_default()
        ),
        cols: cols(&[("name", "Account"), ("state", "State"), ("text", "Why")]),
        rows: m["stragglers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|x| {
                let did = x["did"].as_str().unwrap_or_default();
                let state = x["state"].as_str().unwrap_or_default().to_owned();
                let pill = if state == "failed" { "red" } else { "amber" };
                vec![
                    Cell::new("name", did).link(account_href(did)),
                    Cell::new("state", state).pill(pill),
                    Cell::new("text", x["reason"].as_str().unwrap_or_default()),
                ]
            })
            .collect(),
        empty: "None: every account's document names the new hostname.".into(),
    });
}

async fn redirect_or(s: &Arc<AppState>, op: &Op, r: Result<Value, String>, done: &str) -> Response {
    match r {
        Ok(_) => web::redirect(&format!("/admin/hostname?done={done}")),
        Err(e) => {
            let p = render(s, op, false, None, vec![e]).await;
            web::render(&p, StatusCode::BAD_REQUEST)
        }
    }
}

/// `POST /admin/hostname/prepare` (owners): `owner.hostname.prepare`, then
/// the page runs the pre-checks.
pub async fn prepare(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (op, form) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let r = api(crate::hostnames::prepare(
        State(Arc::clone(&s)),
        op.api_headers(),
        Json(input(json!({"host": web::field(&form, "host")}))),
    )
    .await);
    match r {
        Ok(_) => web::redirect("/admin/hostname?done=prepared&check=1"),
        Err(e) => redirect_or(&s, &op, Err(e), "").await,
    }
}

async fn confirm_render(
    s: &Arc<AppState>,
    op: &Op,
    host: &str,
    old: &str,
    error: Option<String>,
    status: StatusCode,
) -> Response {
    let mut sh = shell(s, op, PageId::Hostname).await;
    sh.live.clear();
    sh.title = format!("Move this server to {host}?");
    web::render(
        &ConfirmPage {
            shell: sh,
            csrf: op.csrf.clone(),
            error,
            what: format!(
                "{host} becomes the hostname now, and the service DID did:web:{host}. Every OAuth session must sign in again. A job moves every did:plc account's PDS endpoint and every hosted did:web document's; accounts it can't move are listed here and in their account portal. {old} keeps full service while any DID document names it."
            ),
            undo: "This isn't reversible in practice once tokens and PLC operations have moved."
                .into(),
            action: CONFIRM_PATH.into(),
            back: PageId::Hostname.href().into(),
            hidden: Vec::<Hidden>::new(),
            typed: Some(host.into()),
            button: "Confirm the migration".into(),
        },
        status,
    )
}

/// The prepared candidate: (new host, old host).
async fn candidate(s: &AppState) -> Option<(String, String)> {
    s.store
        .open_hostname_migration()
        .await
        .ok()
        .flatten()
        .filter(|m| m.state == "prepared")
        .map(|m| (m.new_host, m.old_host))
}

/// `GET /admin/hostname/confirm` (owners, freshly confirmed).
pub async fn confirm_page(State(s): St, headers: HeaderMap) -> Response {
    let op = match require_op(&s, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    if op.role < Role::Owner {
        return crate::console::refused("Confirming a hostname migration needs the owner role.");
    }
    if let Err(r) = need_confirmed(&op, CONFIRM_PATH) {
        return r;
    }
    let Some((host, old)) = candidate(&s).await else {
        return web::redirect(PageId::Hostname.href());
    };
    confirm_render(&s, &op, &host, &old, None, StatusCode::OK).await
}

/// `POST /admin/hostname/confirm`: the typed name and a fresh
/// confirmation, then `owner.hostname.confirm` (which runs the pre-checks
/// again and refuses on a failure).
pub async fn confirm(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (op, form) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = need_confirmed(&op, CONFIRM_PATH) {
        return r;
    }
    let Some((host, old)) = candidate(&s).await else {
        return web::redirect(PageId::Hostname.href());
    };
    if web::field(&form, "typed") != host {
        return confirm_render(
            &s,
            &op,
            &host,
            &old,
            Some(format!("Type {host} to confirm.")),
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    match api(crate::hostnames::confirm(
        State(Arc::clone(&s)),
        op.api_headers(),
        Json(input(json!({}))),
    )
    .await)
    {
        Ok(_) => web::redirect("/admin/hostname?done=confirmed"),
        Err(e) => confirm_render(&s, &op, &host, &old, Some(e), StatusCode::BAD_REQUEST).await,
    }
}

/// `POST /admin/hostname/cancel`: `owner.hostname.cancel`.
pub async fn cancel(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (op, _) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let r = api(
        crate::hostnames::cancel(State(Arc::clone(&s)), op.api_headers(), Json(json!({}))).await,
    );
    redirect_or(&s, &op, r, "cancelled").await
}

/// `POST /admin/hostname/close`: `owner.hostname.close`.
pub async fn close(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (op, _) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let r = api(
        crate::hostnames::close(State(Arc::clone(&s)), op.api_headers(), Json(json!({}))).await,
    );
    redirect_or(&s, &op, r, "closed").await
}

/// The console's lines about hostname migrations: one waiting to be
/// confirmed, one running, and accounts still pointing at an old host.
pub async fn banners(s: &AppState) -> Vec<Banner> {
    let link = || {
        Some(Link {
            href: PageId::Hostname.href().into(),
            label: "Hostname".into(),
        })
    };
    let mut out = Vec::new();
    for m in s.store.hostname_migrations().await.unwrap_or_default() {
        let left = || async { s.store.hostname_straggler_count(&m.id).await.unwrap_or(0) };
        match m.state.as_str() {
            "prepared" => out.push(Banner {
                bad: false,
                text: format!(
                    "A new hostname, {}, is prepared: check it and confirm the migration, or cancel it.",
                    m.new_host
                ),
                link: link(),
            }),
            "running" => {
                let n = left().await;
                let text = if m.job_done_at.is_none() {
                    format!(
                        "The hostname is moving from {} to {}: the job is moving accounts.",
                        m.old_host, m.new_host
                    )
                } else if n > 0 {
                    format!(
                        "The hostname moved from {} to {}; {n} account(s) still point at {}: they're listed for their owners to update.",
                        m.old_host, m.new_host, m.old_host
                    )
                } else {
                    format!(
                        "The hostname moved from {} to {}; the migration ends with its window.",
                        m.old_host, m.new_host
                    )
                };
                out.push(Banner {
                    bad: false,
                    text,
                    link: link(),
                });
            }
            "done" if m.former_at.is_none() => {
                let n = left().await;
                if n > 0 {
                    out.push(Banner {
                        bad: false,
                        text: format!(
                            "{n} account(s) still point at the old hostname {}; it keeps full service for them.",
                            m.old_host
                        ),
                        link: link(),
                    });
                }
            }
            _ => {}
        }
    }
    out
}

/// What an account holder whose DID document still names an old host is
/// told: (the banner line, what to do).
pub async fn straggler_notice(s: &AppState, did: &str) -> Option<(String, String)> {
    let (old, new, state, reason) = s.store.hostname_straggler_of(did).await.ok()??;
    let banner = format!(
        "This server moved from {old} to {new}, but your DID document still names {old} as your PDS. {old} keeps working for you until it's updated."
    );
    let fix = if did.starts_with("did:web:") {
        let host = did.trim_start_matches("did:web:").replace("%3A", ":");
        format!(
            "Your DID document is one you host: edit https://{host}/.well-known/did.json and set the #atproto_pds service endpoint to https://{new}."
        )
    } else if state == "failed" {
        format!(
            "This server tried to update it and failed ({}); the operator sees it listed. You can also update it yourself: sign a PLC operation with one of your rotation keys that sets the atproto_pds endpoint to https://{new}.",
            reason.unwrap_or_default()
        )
    } else {
        format!(
            "Your DID document lists none of this server's rotation keys, so it can't update it for you: sign a PLC operation with one of your own rotation keys that sets the atproto_pds endpoint to https://{new}."
        )
    };
    Some((banner, fix))
}

// ---------------------------------------------------------------- import report

/// `GET /admin/import`: the report of the reference-PDS import this
/// install came from (`viewer.getReferenceImport`).
pub async fn import_report(State(s): St, headers: HeaderMap) -> Response {
    let op = match require_op(&s, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let mut p = crate::console_sections::section(
        &s,
        &op,
        PageId::Import,
        "What `aurora-locus import reference-pds` brought over from a reference PDS installation, and anything it skipped.",
    )
    .await;
    let v =
        match api(
            crate::locus_admin::get_reference_import(State(Arc::clone(&s)), op.api_headers()).await,
        ) {
            Ok(v) => v,
            Err(e) => {
                p.errors.push(e);
                return web::render(&p, StatusCode::OK);
            }
        };
    if v["imported"] != json!(true) {
        p.cards.push(Card {
            title: "Import".into(),
            facts: vec![Fact::new(
                "This install",
                "wasn't imported from a reference PDS",
            )],
        });
        return web::render(&p, StatusCode::OK);
    }
    let total = v["skippedTotal"].as_u64().unwrap_or(0);
    p.cards.push(Card {
        title: "Import".into(),
        facts: vec![
            Fact::new("Imported", v["importedAt"].as_str().unwrap_or_default()),
            Fact::new("Skipped", total.to_string()).class(if total > 0 { "warn" } else { "ok" }),
        ],
    });
    p.tables.push(Table {
        title: "Counts".into(),
        cols: cols(&[("name", "What"), ("num", "How many")]),
        rows: v["counts"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, n)| {
                vec![
                    Cell::new("name", k.clone()),
                    Cell::new("num", n.to_string()),
                ]
            })
            .collect(),
        empty: "Nothing was counted.".into(),
    });
    let shown = v["skipped"].as_array().map_or(0, Vec::len);
    p.tables.push(Table {
        title: if (shown as u64) < total {
            format!("Skipped (the first {shown} of {total})")
        } else {
            "Skipped".into()
        },
        cols: cols(&[("text", "What and why")]),
        rows: v["skipped"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|x| vec![Cell::new("text", x.as_str().unwrap_or_default())])
            .collect(),
        empty: "Nothing was skipped.".into(),
    });
    web::render(&p, StatusCode::OK)
}
