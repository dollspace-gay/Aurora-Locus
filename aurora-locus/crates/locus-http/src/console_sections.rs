//! The console's other sections: moderation, scanning, private posts,
//! identity, network services, email, proxy and TLS, limits, storage, jobs,
//! the audit log and logs. Each is facts, tables and forms over the same
//! settings cards; changes go through the admin API as the operator (or,
//! for SMTP, which the API doesn't carry, the mailer, with the same role
//! check and audit entry).

#![allow(clippy::result_large_err)]

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use locus_core::settings;
use locus_store::email::{Sender, SenderStatus};
use locus_web::console::PageId;
use locus_web::console_pages::{
    Button, Card, Cell, Fact, Form, Input, SectionPage, SettingField, Table,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::admin_auth::Role;
use crate::console::{Op, bytes, form_op, require_op, setting_field, shell, when};
use crate::console_people::account_href;
use crate::state::AppState;
use crate::web;

type St = State<Arc<AppState>>;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/moderation", get(moderation))
        .route("/admin/moderation/status", post(moderation_status))
        .route("/admin/scanning", get(scanning))
        .route("/admin/scanning/resolve", post(resolve_review))
        .route("/admin/private", get(private))
        .route("/admin/private/rotate", post(rotate))
        .route("/admin/identity", get(identity))
        .route("/admin/services", get(services))
        .route("/admin/services/crawl", post(crawl_now))
        .route("/admin/email", get(email))
        .route("/admin/email/{action}", post(email_action))
        .route("/admin/proxy", get(proxy))
        .route("/admin/limits", get(limits))
        .route("/admin/storage", get(storage))
        .route("/admin/jobs", get(jobs))
        .route("/admin/jobs/{action}", post(job_action))
        .route("/admin/shards", get(shards))
        .route("/admin/audit", get(audit))
        .route("/admin/audit/export", get(audit_export))
        .route("/admin/logs", get(logs))
}

fn input<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).expect("the console builds valid API inputs")
}

fn api(r: crate::error::XrpcResult<Json<Value>>) -> Result<Value, String> {
    r.map(|j| j.0).map_err(|e| e.message)
}

/// The settings cards for `keys`.
fn fields(s: &AppState, keys: &[&str]) -> Vec<SettingField> {
    let snap = s.snapshot();
    keys.iter()
        .filter_map(|k| settings::def(k).ok())
        .map(|d| setting_field(&snap, d))
        .collect()
}

/// Every setting under `prefix.`, minus `except`.
fn fields_under(s: &AppState, prefix: &str, except: &[&str]) -> Vec<SettingField> {
    let snap = s.snapshot();
    settings::REGISTRY
        .iter()
        .filter(|d| d.key.starts_with(&format!("{prefix}.")) && !except.contains(&d.key))
        .map(|d| setting_field(&snap, d))
        .collect()
}

pub(crate) async fn section(s: &AppState, op: &Op, page: PageId, intro: &str) -> SectionPage {
    SectionPage {
        shell: shell(s, op, page).await,
        csrf: op.csrf.clone(),
        intro: intro.into(),
        notice: None,
        errors: Vec::new(),
        cards: Vec::new(),
        forms: Vec::new(),
        tables: Vec::new(),
        fields: Vec::new(),
        advanced: Vec::new(),
        pre: None,
        frames: Vec::new(),
        can_edit: op.role >= Role::Admin,
        back: page.href().into(),
    }
}

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

fn or_none(v: &str) -> String {
    if v.is_empty() {
        "none".into()
    } else {
        v.to_owned()
    }
}

fn card(title: &str, facts: Vec<Fact>) -> Card {
    Card {
        title: title.into(),
        facts,
    }
}

fn cols(c: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    c.iter().map(|(k, l)| (*k, (*l).to_owned())).collect()
}

fn button(action: &str, label: &str, hidden: &[(&str, &str)], danger: bool) -> Cell {
    Cell {
        class: "actions",
        button: Some(Button {
            action: action.into(),
            label: label.into(),
            hidden: hidden
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
            danger,
        }),
        ..Cell::default()
    }
}

#[derive(Deserialize, Default)]
pub struct PageQuery {
    #[serde(default)]
    done: Option<String>,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    blob: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    actor: Option<String>,
    #[serde(default)]
    before: Option<i64>,
}

macro_rules! op_or_return {
    ($s:expr, $h:expr) => {
        match require_op(&$s, &$h).await {
            Ok(o) => o,
            Err(r) => return r,
        }
    };
}

// ---------------------------------------------------------------- moderation

/// `GET /admin/moderation?subject=&blob=`: a subject's takedown status,
/// recent takedowns, and the moderation services.
pub async fn moderation(State(s): St, headers: HeaderMap, Query(q): Query<PageQuery>) -> Response {
    let op = op_or_return!(s, headers);
    let mut p = section(&s, &op, PageId::Moderation, "Take down accounts, records and blobs, or reverse it. Reports from people go to the report service below, not here.").await;
    p.notice = q.done.map(|_| "Done.".into());
    let subject = q.subject.unwrap_or_default();
    let blob = q.blob.unwrap_or_default();
    p.forms.push(Form {
        title: "Look up a subject".into(),
        text: "An account's DID, a record's at:// URI, or a DID with a blob's CID.".into(),
        action: "/admin/moderation".into(),
        get: true,
        inputs: vec![
            Input::new("subject", "DID or at:// URI", "text").value(subject.clone()),
            Input::new("blob", "Blob CID (with a DID)", "text").value(blob.clone()),
        ],
        hidden: Vec::new(),
        button: "Look up".into(),
        danger: false,
    });
    if !subject.is_empty() {
        let mut query = vec![];
        if subject.starts_with("at://") {
            query.push(format!(
                "uri={}",
                url::form_urlencoded::byte_serialize(subject.as_bytes()).collect::<String>()
            ));
        } else {
            query.push(format!(
                "did={}",
                url::form_urlencoded::byte_serialize(subject.as_bytes()).collect::<String>()
            ));
            if !blob.is_empty() {
                query.push(format!(
                    "blob={}",
                    url::form_urlencoded::byte_serialize(blob.as_bytes()).collect::<String>()
                ));
            }
        }
        let uri: axum::http::Uri = format!("/?{}", query.join("&")).parse().unwrap_or_default();
        let params = Query::try_from_uri(&uri);
        match params {
            Ok(params) => match api(crate::admin::get_subject_status(
                State(Arc::clone(&s)),
                op.api_headers(),
                params,
            )
            .await)
            {
                Ok(v) => {
                    let applied = v["takedown"]["applied"].as_bool().unwrap_or(false);
                    let mut facts = vec![Fact::new("Subject", subject.clone())];
                    if !blob.is_empty() {
                        facts.push(Fact::new("Blob", blob.clone()));
                    }
                    facts.push(
                        Fact::new("Taken down", if applied { "yes" } else { "no" })
                            .class(if applied { "bad" } else { "ok" }),
                    );
                    if let Some(r) = v["takedown"]["ref"].as_str() {
                        facts.push(Fact::new("Reference", r));
                    }
                    p.cards.push(card("Status", facts));
                    if op.role >= Role::Moderator {
                        let mut hidden = vec![
                            ("subject".to_owned(), subject.clone()),
                            ("blob".to_owned(), blob.clone()),
                        ];
                        hidden.push(("applied".into(), (!applied).to_string()));
                        p.forms.push(Form {
                            title: if applied {
                                "Reverse the takedown".into()
                            } else {
                                "Take it down".into()
                            },
                            text: String::new(),
                            action: "/admin/moderation/status".into(),
                            get: false,
                            inputs: if applied {
                                Vec::new()
                            } else {
                                vec![Input::new("ref", "Reference (optional)", "text")]
                            },
                            hidden,
                            button: if applied {
                                "Reverse".into()
                            } else {
                                "Take down".into()
                            },
                            danger: !applied,
                        });
                    }
                }
                Err(e) => p.errors.push(e),
            },
            Err(e) => p.errors.push(e.to_string()),
        }
    }
    let audit = s.store.audit_entries(None, 500).await.unwrap_or_default();
    p.tables.push(Table {
        title: "Recent takedowns and reversals".into(),
        cols: cols(&[("when", "When"), ("name", "By"), ("text", "What")]),
        rows: audit
            .into_iter()
            .filter(|e| e.action == "admin.updateSubjectStatus")
            .take(50)
            .map(|e| {
                vec![
                    Cell::new("when", when(e.at)),
                    Cell::new("name", e.actor),
                    Cell::new("text", e.detail_json.unwrap_or_default()).code(),
                ]
            })
            .collect(),
        empty: "None yet.".into(),
    });
    let snap = s.snapshot();
    p.cards.push(card(
        "Moderation services",
        vec![
            Fact::new(
                "Moderation service",
                or_none(snap.text("moderation.service_url")),
            ),
            Fact::new("Its DID", or_none(snap.text("moderation.service_did"))),
            Fact::new(
                "Reports go to",
                or_none(snap.text("moderation.report_service_url")),
            ),
        ],
    ));
    p.fields = fields_under(&s, "moderation", &[]);
    web::render(&p, StatusCode::OK)
}

/// `POST /admin/moderation/status`.
pub async fn moderation_status(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (op, form) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let subject = web::field(&form, "subject");
    let blob = web::field(&form, "blob");
    let applied = web::field(&form, "applied") == "true";
    let subject_ref = if subject.starts_with("at://") {
        json!({"$type": "com.atproto.repo.strongRef", "uri": subject, "cid": ""})
    } else if !blob.is_empty() {
        json!({"$type": "com.atproto.admin.defs#repoBlobRef", "did": subject, "cid": blob})
    } else {
        json!({"$type": "com.atproto.admin.defs#repoRef", "did": subject})
    };
    let mut takedown = json!({"applied": applied});
    let r = web::field(&form, "ref");
    if applied && !r.is_empty() {
        takedown["ref"] = json!(r);
    }
    let back = format!(
        "/admin/moderation?subject={}&blob={}",
        url::form_urlencoded::byte_serialize(subject.as_bytes()).collect::<String>(),
        url::form_urlencoded::byte_serialize(blob.as_bytes()).collect::<String>()
    );
    match api(crate::admin::update_subject_status(
        State(Arc::clone(&s)),
        op.api_headers(),
        Json(input(json!({"subject": subject_ref, "takedown": takedown}))),
    )
    .await)
    {
        Ok(_) => web::redirect(&format!("{back}&done=1")),
        Err(e) => {
            let mut p = section(&s, &op, PageId::Moderation, "").await;
            p.errors.push(e);
            web::render(&p, StatusCode::BAD_REQUEST)
        }
    }
}

// ---------------------------------------------------------------- scanning

/// `GET /admin/scanning`: the scanner, and its matches waiting for review.
pub async fn scanning(State(s): St, headers: HeaderMap, Query(q): Query<PageQuery>) -> Response {
    let op = op_or_return!(s, headers);
    let mut p = section(&s, &op, PageId::Scanning, "Uploads can go through a scanner before they're stored. A match is quarantined and waits here: clear a false positive to release it, or mark it reported to keep it held.").await;
    p.notice = q.done.map(|_| "Done.".into());
    let snap = s.snapshot();
    let counts = s.store.console_counts().await.unwrap_or_default();
    p.cards.push(card(
        "Scanner",
        vec![
            Fact::new("Scanner", snap.text("blobs.scanner")).class(
                if snap.text("blobs.scanner") == "none" {
                    "warn"
                } else {
                    "ok"
                },
            ),
            Fact::new("Its URL", or_none(snap.text("blobs.scanner_url"))),
            Fact::new("When it's down", snap.text("blobs.scan_unavailable")),
            Fact::new("Quarantined blobs", counts.quarantined.to_string()),
        ],
    ));
    let rows = s.store.reviews("open", 200).await.unwrap_or_default();
    p.tables.push(Table {
        title: "Waiting for review".into(),
        cols: cols(&[
            ("when", "Found"),
            ("name", "Account"),
            ("text", "Blob"),
            ("state", "Scanner said"),
            ("text", "Why"),
            ("actions", ""),
            ("actions", ""),
        ]),
        rows: rows
            .into_iter()
            .map(|r| {
                let id = r.id.to_string();
                let mut row = vec![
                    Cell::new("when", when(r.created_at)),
                    Cell::new("name", r.did.clone()).link(account_href(&r.did)),
                    Cell::new("text", r.cid).code(),
                    Cell::new("state", r.result).pill("amber"),
                    Cell::new("text", r.reason.unwrap_or_default()),
                ];
                if op.role >= Role::Moderator {
                    row.push(button(
                        "/admin/scanning/resolve",
                        "Clear (release)",
                        &[("id", &id), ("status", "cleared")],
                        false,
                    ));
                    row.push(button(
                        "/admin/scanning/resolve",
                        "Reported (keep held)",
                        &[("id", &id), ("status", "reported")],
                        true,
                    ));
                }
                row
            })
            .collect(),
        empty: "Nothing waits for review.".into(),
    });
    p.fields = fields(
        &s,
        &[
            "blobs.scanner",
            "blobs.scanner_url",
            "blobs.scan_unavailable",
        ],
    );
    web::render(&p, StatusCode::OK)
}

/// `POST /admin/scanning/resolve`.
pub async fn resolve_review(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (op, form) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let id: i64 = web::field(&form, "id").parse().unwrap_or(-1);
    match api(crate::locus_admin::resolve_review(
        State(Arc::clone(&s)),
        op.api_headers(),
        Json(input(
            json!({"id": id, "status": web::field(&form, "status")}),
        )),
    )
    .await)
    {
        Ok(_) => web::redirect("/admin/scanning?done=1"),
        Err(e) => {
            let mut p = section(&s, &op, PageId::Scanning, "").await;
            p.errors.push(e);
            web::render(&p, StatusCode::BAD_REQUEST)
        }
    }
}

// ---------------------------------------------------------------- private posts

/// `GET /admin/private`: kryphocron's state, rotation and audit events.
pub async fn private(State(s): St, headers: HeaderMap, Query(q): Query<PageQuery>) -> Response {
    let op = op_or_return!(s, headers);
    let mut p = section(&s, &op, PageId::Private, "Private posts are stored encoded and shown only to the audience their author picks. Their encoding rotates on a schedule.").await;
    p.notice = q.done.map(|_| {
        "A new generation started; posts are re-encoded under it over the rotation period.".into()
    });
    match api(crate::locus_admin::kryphocron_status(State(Arc::clone(&s)), op.api_headers()).await)
    {
        Ok(v) => {
            p.cards.push(card(
                "Private tier",
                vec![
                    {
                        let enabled = v["privateTierEnabled"].as_bool().unwrap_or(false);
                        let has_private = v["privateAppview"].is_string();
                        let f = Fact::new(
                            "Private posts",
                            match (enabled, has_private) {
                                (true, true) => "on",
                                (true, false) => "off: no private AppView is set",
                                (false, _) => "off",
                            },
                        );
                        if enabled && !has_private {
                            f.class("bad")
                        } else {
                            f
                        }
                    },
                    Fact::new(
                        "Private AppView",
                        v["privateAppview"]
                            .as_str()
                            .map_or_else(|| "none".into(), str::to_owned),
                    ),
                    Fact::new(
                        "Prism",
                        if v["prism"]["down"].as_bool().unwrap_or(false) {
                            "unreachable"
                        } else {
                            "reachable"
                        },
                    )
                    .class(if v["prism"]["down"].as_bool().unwrap_or(false) {
                        "bad"
                    } else {
                        "ok"
                    }),
                    Fact::new(
                        "Media gate",
                        format!(
                            "{} (in effect: {})",
                            on_off(v["mediaGate"]["setting"].as_bool().unwrap_or(false)),
                            if v["mediaGate"]["effective"].as_bool().unwrap_or(false) {
                                "yes"
                            } else {
                                "no"
                            }
                        ),
                    ),
                    Fact::new("Audit events dropped", v["auditRejected"].to_string()),
                ],
            ));
            p.cards.push(card(
                "Rotation",
                vec![
                    Fact::new("Every", format!("{} days", v["rotation"]["periodDays"])),
                    Fact::new(
                        "Current generation",
                        v["rotation"]["generation"]
                            .as_str()
                            .map_or_else(|| "none yet".into(), str::to_owned),
                    ),
                    Fact::new(
                        "Since",
                        v["rotation"]["generatedAt"].as_str().unwrap_or("—"),
                    ),
                ],
            ));
            let flagged = v["flaggedRecords"].as_array().cloned().unwrap_or_default();
            p.tables.push(Table {
                title: "Records that couldn't be re-encoded".into(),
                cols: cols(&[("text", "Record"), ("text", "Why")]),
                rows: flagged
                    .iter()
                    .map(|r| {
                        vec![
                            Cell::new("text", r["uri"].as_str().unwrap_or_default()).code(),
                            Cell::new("text", r["why"].as_str().unwrap_or_default()),
                        ]
                    })
                    .collect(),
                empty: "None.".into(),
            });
        }
        Err(e) => p.errors.push(e),
    }
    if op.role >= Role::Admin {
        p.forms.push(Form {
            title: "Rotate now".into(),
            text: "Starts a new generation at once instead of waiting for the schedule.".into(),
            action: "/admin/private/rotate".into(),
            get: false,
            inputs: Vec::new(),
            hidden: Vec::new(),
            button: "Start a new generation".into(),
            danger: false,
        });
    }
    let events = s.store.kr_audit(100).await.unwrap_or_default();
    p.tables.push(Table {
        title: "Audit events".into(),
        cols: cols(&[
            ("when", "When"),
            ("name", "Sink"),
            ("name", "Event"),
            ("text", "Detail"),
        ]),
        rows: events
            .into_iter()
            .map(|e| {
                vec![
                    Cell::new("when", when(e.at)),
                    Cell::new("name", e.sink),
                    Cell::new("name", e.event),
                    Cell::new("text", e.detail_json).code(),
                ]
            })
            .collect(),
        empty: "None yet.".into(),
    });
    p.fields = fields(
        &s,
        &[
            "kryphocron.private_tier_enabled",
            "kryphocron.private_media_gated",
            "kryphocron.media_trusted_services",
            "kryphocron.rotation_period_days",
        ],
    );
    p.advanced = fields_under(
        &s,
        "kryphocron",
        &[
            "kryphocron.private_tier_enabled",
            "kryphocron.private_media_gated",
            "kryphocron.media_trusted_services",
            "kryphocron.rotation_period_days",
        ],
    );
    web::render(&p, StatusCode::OK)
}

/// `POST /admin/private/rotate`.
pub async fn rotate(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (op, _) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    match api(crate::locus_admin::rotate_now(State(Arc::clone(&s)), op.api_headers()).await) {
        Ok(_) => web::redirect("/admin/private?done=1"),
        Err(e) => {
            let mut p = section(&s, &op, PageId::Private, "").await;
            p.errors.push(e);
            web::render(&p, StatusCode::BAD_REQUEST)
        }
    }
}

// ---------------------------------------------------------------- identity

/// `GET /admin/identity`: handles, the service's DID document and the PLC
/// rotation keys.
pub async fn identity(State(s): St, headers: HeaderMap) -> Response {
    let op = op_or_return!(s, headers);
    let mut p = section(
        &s,
        &op,
        PageId::Identity,
        "How accounts here are named and identified.",
    )
    .await;
    let snap = s.snapshot();
    p.cards.push(card(
        "This server",
        vec![
            Fact::new("Hostname", s.hostname.clone()),
            Fact::new("Service DID", s.service_did.clone()),
            Fact::new("Handle domains", s.handle_domains().join(", ")),
            Fact::new(
                "Reserved handles",
                or_none(&snap.list("identity.reserved_handles").join(", ")),
            ),
            Fact::new("PLC directory", snap.text("identity.plc_url")),
            Fact::new(
                "New accounts get",
                format!("did:{}", snap.text("accounts.default_did_method")),
            ),
        ],
    ));
    p.tables.push(Table {
        title: "PLC rotation keys".into(),
        cols: cols(&[("text", "Key"), ("state", "Role")]),
        rows: s
            .secrets
            .plc_keys
            .held_keys()
            .into_iter()
            .map(|(k, role)| {
                vec![
                    Cell::new("text", k).code(),
                    Cell::new("state", role.as_str()).pill(if role.as_str() == "current" {
                        "on"
                    } else {
                        ""
                    }),
                ]
            })
            .collect(),
        empty: "None.".into(),
    });
    let doc = serde_json::to_string_pretty(&crate::identity::service_document(&s).to_json())
        .unwrap_or_default();
    p.pre = Some(("The service's DID document".into(), doc));
    p.fields = fields(
        &s,
        &[
            "identity.handle_domains",
            "identity.reserved_handles",
            "identity.recovery_did_key",
        ],
    );
    p.advanced = fields_under(
        &s,
        "identity",
        &[
            "identity.handle_domains",
            "identity.reserved_handles",
            "identity.recovery_did_key",
        ],
    );
    web::render(&p, StatusCode::OK)
}

// ---------------------------------------------------------------- services

/// `GET /admin/services`: the AppView, relays and the firehose.
pub async fn services(State(s): St, headers: HeaderMap, Query(q): Query<PageQuery>) -> Response {
    let op = op_or_return!(s, headers);
    let mut p = section(&s, &op, PageId::Services, "The services this PDS talks to: the AppView apps read from, and the relays that carry its posts to the network.").await;
    p.notice = q.done;
    let snap = s.snapshot();
    let (subs, addrs) = s.limits.subscriber_count();
    let seq = s
        .store
        .seq_state()
        .await
        .map(|st| st.last_seq)
        .unwrap_or_default();
    p.cards.push(card(
        "AppView",
        vec![
            Fact::new("URL", or_none(snap.text("appview.url"))),
            Fact::new("DID", or_none(snap.text("appview.did"))),
        ],
    ));
    p.cards.push(card(
        "Private AppView",
        vec![
            Fact::new("URL", or_none(snap.text("appview.private_url"))),
            Fact::new("DID", or_none(snap.text("appview.private_did"))),
        ],
    ));
    p.cards.push(card(
        "Firehose",
        vec![
            Fact::new("Sequence", seq.to_string()),
            Fact::new(
                "Connections on this node",
                format!("{subs} from {addrs} address(es)"),
            ),
            Fact::new("Relays", or_none(&snap.list("sync.crawlers").join(", "))),
        ],
    ));
    if op.role >= Role::Admin {
        p.forms.push(Form {
            title: "Ask the relays to crawl".into(),
            text: "Relays are asked after new events anyway; this asks every one now.".into(),
            action: "/admin/services/crawl".into(),
            get: false,
            inputs: Vec::new(),
            hidden: Vec::new(),
            button: "Request a crawl now".into(),
            danger: false,
        });
    }
    p.fields = fields(
        &s,
        &[
            "appview.url",
            "appview.did",
            "appview.private_url",
            "appview.private_did",
            "sync.crawlers",
        ],
    );
    let mut adv = fields(&s, &["appview.cdn_url_pattern"]);
    adv.extend(fields_under(&s, "proxy", &[]));
    adv.extend(fields_under(&s, "oauth", &[]));
    adv.extend(fields_under(
        &s,
        "sync",
        &[
            "sync.crawlers",
            "sync.max_subscribers",
            "sync.max_subscribers_per_ip",
        ],
    ));
    p.advanced = adv;
    web::render_shell(&p, &p.shell, StatusCode::OK)
}

/// `POST /admin/services/crawl` (admins).
pub async fn crawl_now(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (op, _) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if op.role < Role::Admin {
        return (StatusCode::FORBIDDEN, "This needs the admin role").into_response();
    }
    let crawlers: Vec<String> = s
        .snapshot()
        .list("sync.crawlers")
        .into_iter()
        .map(str::to_owned)
        .collect();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default();
    let mut said = Vec::new();
    for c in &crawlers {
        match locus_sync::request_crawl(&http, c, &s.hostname).await {
            Ok(()) => said.push(format!("{c}: asked")),
            Err(e) => said.push(format!("{c}: {e}")),
        }
    }
    let _ = s
        .store
        .audit(
            &op.actor(),
            "network.requestCrawl",
            None,
            Some(&json!({"crawlers": crawlers})),
        )
        .await;
    let msg = if said.is_empty() {
        "No relays are set.".to_owned()
    } else {
        said.join("; ")
    };
    web::redirect(&format!(
        "/admin/services?done={}",
        url::form_urlencoded::byte_serialize(msg.as_bytes()).collect::<String>()
    ))
}

// ---------------------------------------------------------------- email

const TEST_CODE_SECRET: &str = "email/console_test_code";

fn status_text(st: Option<SenderStatus>) -> (&'static str, &'static str) {
    match st {
        Some(SenderStatus::Verified) => ("working: a test email arrived", "ok"),
        Some(SenderStatus::Unverified) => ("connected; delivery not confirmed", "warn"),
        Some(SenderStatus::Failed) => ("the SMTP server refuses the connection", "bad"),
        None => ("not set up", "warn"),
    }
}

/// `GET /admin/email`.
pub async fn email(State(s): St, headers: HeaderMap, Query(q): Query<PageQuery>) -> Response {
    let op = op_or_return!(s, headers);
    email_render(&s, &op, q.done, Vec::new(), StatusCode::OK).await
}

async fn email_render(
    s: &AppState,
    op: &Op,
    notice: Option<String>,
    errors: Vec<String>,
    status: StatusCode,
) -> Response {
    let mut p = section(s, op, PageId::Email, "Email sends sign-up confirmations, password resets, sign-in codes and moderators' messages. The SMTP URLs carry passwords, so they're stored encrypted and never shown.").await;
    p.notice = notice;
    p.errors = errors;
    let (main, mclass) = status_text(s.mailer.status(Sender::Main).await.ok().flatten());
    let (moderation, modclass) =
        status_text(s.mailer.status(Sender::Moderation).await.ok().flatten());
    let snap = s.snapshot();
    p.cards.push(card(
        "Senders",
        vec![
            Fact::new("Main", main).class(mclass),
            Fact::new("Sends from", or_none(snap.text("email.from_address"))),
            Fact::new(
                "Moderation",
                if moderation == "not set up" {
                    "uses the main sender"
                } else {
                    moderation
                },
            )
            .class(if moderation == "not set up" {
                ""
            } else {
                modclass
            }),
            Fact::new(
                "Moderation sends from",
                or_none(snap.text("email.moderation_from_address")),
            ),
        ],
    ));
    if op.role >= Role::Admin {
        let sender = || Input {
            options: vec![
                locus_web::setup::Choice {
                    value: "main".into(),
                    label: "Main".into(),
                    checked: true,
                },
                locus_web::setup::Choice {
                    value: "moderation".into(),
                    label: "Moderation".into(),
                    checked: false,
                },
            ],
            ..Input::new("sender", "Sender", "select")
        };
        // One form: the provider, then only its fields. Saving stores the
        // sender's SMTP URL and checks the connection and sign-in at once.
        p.forms.push(Form {
            title: "How email is sent".into(),
            text: "Pick your email provider and enter what it gives you. For any other provider, pick Other and enter its SMTP server's details.".into(),
            action: "/admin/email/smtp".into(),
            get: false,
            inputs: std::iter::once(sender()).chain(email_inputs()).collect(),
            hidden: Vec::new(),
            button: "Save and check".into(),
            danger: false,
        });
        p.forms.push(Form {
            title: "Send a test email".into(),
            text: "From the main sender. Enter the code it carries below to confirm delivery."
                .into(),
            action: "/admin/email/test".into(),
            get: false,
            inputs: vec![Input::new("to", "To", "email")],
            hidden: Vec::new(),
            button: "Send".into(),
            danger: false,
        });
        p.forms.push(Form {
            title: "Confirm delivery".into(),
            text: String::new(),
            action: "/admin/email/verify".into(),
            get: false,
            inputs: vec![Input::new("code", "The code from the test email", "text")],
            hidden: Vec::new(),
            button: "Confirm".into(),
            danger: false,
        });
    }
    p.tables.push(Table {
        title: "The emails this PDS sends".into(),
        cols: cols(&[("name", "Email"), ("text", "Subject")]),
        rows: [
            ("Confirm email", "Email Confirmation"),
            ("Change email", "Email Update Requested"),
            ("Reset password", "Password Reset Requested"),
            ("Delete account", "Account Deletion Requested"),
            ("Identity update", "PLC Update Operation Requested"),
            ("Sign-in code", "Sign-in Confirmation"),
            ("Test", "Test email"),
        ]
        .into_iter()
        .map(|(n, subj)| vec![Cell::new("name", n), Cell::new("text", subj)])
        .collect(),
        empty: String::new(),
    });
    p.fields = fields_under(s, "email", &[]);
    web::render(&p, status)
}

/// The email form's inputs: "Who's your email provider?", then every
/// provider's fields, each shown only for its choice (the console's Email
/// page and the setup wizard).
#[must_use]
pub fn email_inputs() -> Vec<Input> {
    use locus_mail::provider::{form_fields, provider_options};
    let choices = |opts: &[(&str, &str)]| {
        opts.iter()
            .enumerate()
            .map(|(i, (v, l))| locus_web::setup::Choice {
                value: (*v).into(),
                label: (*l).into(),
                checked: i == 0,
            })
            .collect()
    };
    std::iter::once(Input {
        options: choices(&provider_options()),
        ..Input::new("provider", "Who's your email provider?", "select")
    })
    .chain(form_fields().into_iter().map(|f| Input {
        name: f.name,
        label: f.label,
        kind: f.kind,
        value: f.value,
        options: choices(&f.options),
        hint: f.hint,
        show_for: f.show_for,
    }))
    .collect()
}

fn code_hash(code: &str) -> String {
    hex::encode(Sha256::digest(
        format!("locus-console-test:{}", code.trim().to_ascii_uppercase()).as_bytes(),
    ))
}

/// `POST /admin/email/{smtp|test|verify}` (admins).
pub async fn email_action(
    State(s): St,
    headers: HeaderMap,
    axum::extract::Path(action): axum::extract::Path<String>,
    body: Bytes,
) -> Response {
    let (op, form) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if op.role < Role::Admin {
        return email_render(
            &s,
            &op,
            None,
            vec!["Email settings need the admin role.".into()],
            StatusCode::FORBIDDEN,
        )
        .await;
    }
    match action.as_str() {
        "smtp" => {
            let sender = if web::field(&form, "sender") == "moderation" {
                Sender::Moderation
            } else {
                Sender::Main
            };
            // Resend's key, a server's details or a URL (`url`, the field's
            // older name, still counts as one).
            let choice = locus_mail::provider::SmtpChoice::from_form(|k| {
                let v = web::field(&form, k);
                if k == "smtp_url" && v.is_empty() {
                    web::field(&form, "url").to_owned()
                } else {
                    v.to_owned()
                }
            });
            let url = match choice.smtp_url() {
                Ok(u) => u,
                Err(e) => {
                    return email_render(&s, &op, None, vec![e], StatusCode::BAD_REQUEST).await;
                }
            };
            let status = s.mailer.set_url(sender, &url).await;
            let _ = s
                .store
                .audit(&op.actor(), "email.setSmtp", Some(sender.as_str()), None)
                .await;
            match status {
                Ok(SenderStatus::Failed) => {
                    let msg = match s.mailer.blocked_port_hint(sender).await {
                        Some(h) => format!("Saved, but the SMTP server couldn't be reached. {h}"),
                        None => "Saved, but the SMTP server refused the connection.".into(),
                    };
                    email_render(&s, &op, None, vec![msg], StatusCode::BAD_REQUEST).await
                }
                Ok(_) => {
                    email_render(
                        &s,
                        &op,
                        Some("Saved; the SMTP server accepted the connection.".into()),
                        Vec::new(),
                        StatusCode::OK,
                    )
                    .await
                }
                Err(e) => {
                    email_render(
                        &s,
                        &op,
                        None,
                        vec![crate::console::server_problem(&e)],
                        StatusCode::INTERNAL_SERVER_ERROR,
                    )
                    .await
                }
            }
        }
        "test" => {
            let to = web::field(&form, "to").to_owned();
            let code = crate::email::new_token();
            let from = s.snapshot().text("email.from_address").to_owned();
            let service = crate::console::brand(&s);
            let mail =
                locus_mail::Template::TestEmail.render(&[("service", &service), ("token", &code)]);
            let stored = format!(
                "{}|{}",
                code_hash(&code),
                locus_core::time::now_ms() + 15 * 60_000
            );
            let kept = async {
                let mut tx = s.store.begin().await?;
                tx.put_secret(TEST_CODE_SECRET, stored.as_bytes(), &s.master_key)
                    .await?;
                tx.commit().await
            }
            .await;
            if let Err(e) = kept {
                return email_render(
                    &s,
                    &op,
                    None,
                    vec![crate::console::server_problem(&e)],
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
                .await;
            }
            match s.mailer.send(Sender::Main, &from, &to, mail).await {
                Ok(()) => {
                    email_render(
                        &s,
                        &op,
                        Some(format!("Sent to {to}. Enter its code below.")),
                        Vec::new(),
                        StatusCode::OK,
                    )
                    .await
                }
                Err(e) => {
                    email_render(
                        &s,
                        &op,
                        None,
                        vec![format!("Sending failed: {e}")],
                        StatusCode::BAD_REQUEST,
                    )
                    .await
                }
            }
        }
        "verify" => {
            let stored = s
                .store
                .get_secret(TEST_CODE_SECRET, &s.master_key)
                .await
                .ok()
                .flatten();
            let ok = stored
                .and_then(|b| String::from_utf8(b.to_vec()).ok())
                .and_then(|v| {
                    let (hash, exp) = v.split_once('|')?;
                    Some(
                        exp.parse::<i64>().ok()? > locus_core::time::now_ms()
                            && web::ct_eq(&code_hash(web::field(&form, "code")), hash),
                    )
                })
                .unwrap_or(false);
            if !ok {
                return email_render(
                    &s,
                    &op,
                    None,
                    vec!["That code doesn't match (or it expired; send another).".into()],
                    StatusCode::BAD_REQUEST,
                )
                .await;
            }
            let _ = s.store.delete_secret(TEST_CODE_SECRET).await;
            let _ = s.mailer.mark_verified(Sender::Main).await;
            let _ = s
                .store
                .audit(&op.actor(), "email.verified", Some("main"), None)
                .await;
            email_render(
                &s,
                &op,
                Some("The test email arrived: email works.".into()),
                Vec::new(),
                StatusCode::OK,
            )
            .await
        }
        _ => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

// ---------------------------------------------------------------- proxy and TLS

/// `GET /admin/proxy?name=`: how requests arrive, and a DNS check.
pub async fn proxy(State(s): St, headers: HeaderMap, Query(q): Query<PageQuery>) -> Response {
    let op = op_or_return!(s, headers);
    let mut p = section(&s, &op, PageId::Proxy, "How requests reach this PDS, which proxies may say who a visitor is, and whether names resolve.").await;
    let snap = s.snapshot();
    p.cards.push(card(
        "Requests",
        vec![
            Fact::new(
                "Proxy",
                locus_core::setup::ProxyMode::parse(snap.text("deploy.proxy_mode"))
                    .unwrap_or_default()
                    .label(),
            ),
            Fact::new(
                "Trusted proxies",
                snap.list("http.trusted_proxies").join(", "),
            ),
            Fact::new("Public URL", s.public_url.clone()),
        ],
    ));
    let name = q.name.unwrap_or_default();
    p.forms.push(Form {
        title: "Check a name's DNS".into(),
        text: "What a name resolves to from this server.".into(),
        action: "/admin/proxy".into(),
        get: true,
        inputs: vec![
            Input::new("name", "Name", "text").value(if name.is_empty() {
                s.hostname.clone()
            } else {
                name.clone()
            }),
        ],
        hidden: Vec::new(),
        button: "Check".into(),
        danger: false,
    });
    if !name.is_empty() {
        let host = locus_core::setup::normalize_host(&name);
        let facts = match tokio::time::timeout(
            Duration::from_secs(5),
            tokio::net::lookup_host((host.as_str(), 443)),
        )
        .await
        {
            Ok(Ok(addrs)) => {
                let mut ips: Vec<String> = addrs.map(|a| a.ip().to_string()).collect();
                ips.sort();
                ips.dedup();
                vec![
                    Fact::new(
                        host.clone(),
                        if ips.is_empty() {
                            "no addresses".into()
                        } else {
                            ips.join(", ")
                        },
                    )
                    .class(if ips.is_empty() { "bad" } else { "ok" }),
                ]
            }
            Ok(Err(e)) => {
                vec![Fact::new(host.clone(), format!("doesn't resolve: {e}")).class("bad")]
            }
            Err(_) => vec![Fact::new(host.clone(), "no answer in 5 seconds").class("bad")],
        };
        p.cards.push(card("DNS", facts));
    }
    p.fields = fields(&s, &["deploy.proxy_mode", "http.trusted_proxies"]);
    p.advanced = fields(&s, &["http.internal_hosts", "http.request_deadline_ms"]);
    web::render(&p, StatusCode::OK)
}

// ---------------------------------------------------------------- limits

/// `GET /admin/limits`.
pub async fn limits(State(s): St, headers: HeaderMap) -> Response {
    let op = op_or_return!(s, headers);
    let mut p = section(
        &s,
        &op,
        PageId::Limits,
        "How much people may send and how often. Every change applies at once on every node.",
    )
    .await;
    let mut f = fields_under(&s, "limits", &[]);
    f.extend(fields(
        &s,
        &[
            "blobs.upload_limit",
            "sync.max_subscribers",
            "sync.max_subscribers_per_ip",
        ],
    ));
    p.fields = f;
    p.advanced = fields_under(&s, "rate_limits", &[]);
    web::render(&p, StatusCode::OK)
}

// ---------------------------------------------------------------- storage

/// `GET /admin/storage`.
pub async fn storage(State(s): St, headers: HeaderMap) -> Response {
    let op = op_or_return!(s, headers);
    let mut p = section(&s, &op, PageId::Storage, "Where this PDS keeps its data. The backend is fixed at start-up; moving is done with aurora-locus migrate.").await;
    let counts = s.store.console_counts().await.unwrap_or_default();
    p.cards.push(card(
        "Backends",
        vec![
            Fact::new(
                "Database",
                if s.store.is_postgres() {
                    "Postgres"
                } else {
                    "SQLite"
                },
            ),
            Fact::new("Blob storage", "On disk"),
        ],
    ));
    p.cards.push(card(
        "Use",
        vec![
            Fact::new("Records", counts.records.to_string()),
            Fact::new("Blobs", counts.blobs.to_string()),
            Fact::new("Blob bytes", bytes(counts.blob_bytes)),
            Fact::new("Quarantined", counts.quarantined.to_string()),
        ],
    ));
    let jobs = s.store.job_states().await.unwrap_or_default();
    p.tables.push(Table {
        title: "Clean-up".into(),
        cols: cols(&[
            ("name", "Job"),
            ("when", "Last run"),
            ("state", "State"),
            ("text", "Last error"),
        ]),
        rows: jobs
            .into_iter()
            .filter(|j| j.name.starts_with("blob-") || j.name == "gc-blocks")
            .map(|j| {
                vec![
                    Cell::new("name", j.name),
                    Cell::new("when", j.last_run_at.map_or_else(|| "never".into(), when)),
                    Cell::new(
                        "state",
                        if j.last_error.is_some() {
                            "failing"
                        } else {
                            "ok"
                        },
                    )
                    .pill(if j.last_error.is_some() {
                        "red"
                    } else {
                        "fine"
                    }),
                    Cell::new("text", j.last_error.unwrap_or_default()),
                ]
            })
            .collect(),
        empty: "No clean-up has run yet.".into(),
    });
    match api(crate::backups::list(State(Arc::clone(&s)), op.api_headers()).await) {
        Ok(v) => p.tables.push(Table {
            title: "Backups".into(),
            cols: cols(&[
                ("when", "Started"),
                ("when", "Finished"),
                ("state", "State"),
                ("num", "Objects"),
                ("num", "Missing"),
                ("name", "By"),
                ("text", "Where"),
                ("text", "Error"),
            ]),
            rows: v["backups"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|b| {
                    let status = b["status"].as_str().unwrap_or_default().to_owned();
                    let pill = match status.as_str() {
                        "done" => "fine",
                        "failed" => "red",
                        _ => "amber",
                    };
                    let at = |k: &str| b[k].as_i64().map_or_else(|| "—".into(), when);
                    vec![
                        Cell::new("when", at("startedAt")),
                        Cell::new("when", at("finishedAt")),
                        Cell::new("state", status).pill(pill),
                        Cell::new("num", b["objects"].to_string()),
                        Cell::new("num", b["missing"].to_string()),
                        Cell::new("name", b["by"].as_str().unwrap_or_default()),
                        Cell::new("text", b["dir"].as_str().unwrap_or_default()).code(),
                        Cell::new("text", b["error"].as_str().unwrap_or_default()),
                    ]
                })
                .collect(),
            empty: "No backup yet: aurora-locus backup create.".into(),
        }),
        Err(e) => p.errors.push(e),
    }
    p.fields = fields_under(
        &s,
        "blobs",
        &[
            "blobs.scanner",
            "blobs.scanner_url",
            "blobs.scan_unavailable",
            "blobs.upload_limit",
        ],
    );
    web::render_shell(&p, &p.shell, StatusCode::OK)
}

// ---------------------------------------------------------------- jobs

/// `GET /admin/jobs`: every job, who runs it now, and how it went.
pub async fn jobs(State(s): St, headers: HeaderMap, Query(q): Query<PageQuery>) -> Response {
    let op = op_or_return!(s, headers);
    let p = jobs_page(&s, &op, q.done, Vec::new()).await;
    web::render_shell(&p, &p.shell, StatusCode::OK)
}

async fn jobs_page(
    s: &AppState,
    op: &Op,
    notice: Option<String>,
    errors: Vec<String>,
) -> SectionPage {
    let mut p = section(
        s,
        op,
        PageId::Jobs,
        "Background jobs each run on one node at a time; the node holding a job's lease runs it. A paused job stops within seconds and isn't taken up again until it's resumed.",
    )
    .await;
    p.notice = notice;
    p.errors = errors;
    let now = locus_core::time::now_ms();
    let leases = s.store.job_leases().await.unwrap_or_default();
    let states = s.store.job_states().await.unwrap_or_default();
    let controls = s.store.job_controls().await.unwrap_or_default();
    let can_run = op.role >= Role::Admin;
    let mut names: Vec<String> = states.iter().map(|j| j.name.clone()).collect();
    for (n, _, _) in &leases {
        if !names.contains(n) && !n.starts_with(locus_store::env::CLOCK_SLOT_FAMILY) {
            names.push(n.clone());
        }
    }
    for (n, _) in &controls {
        if !names.contains(n) {
            names.push(n.clone());
        }
    }
    names.sort();
    let mut job_cols = vec![
        ("name", "Job"),
        ("name", "Running on"),
        ("when", "Last run"),
        ("when", "Last success"),
        ("num", "Runs"),
        ("state", "State"),
        ("text", "Last error"),
    ];
    if can_run {
        job_cols.extend([("actions", ""), ("actions", "")]);
    }
    p.tables.push(Table {
        title: "Jobs".into(),
        cols: cols(&job_cols),
        rows: names
            .into_iter()
            .map(|n| {
                let st = states.iter().find(|j| j.name == n);
                let holder = leases
                    .iter()
                    .find(|(l, _, exp)| *l == n && *exp > now)
                    .map_or_else(|| "nobody".to_owned(), |(_, h, _)| h.clone());
                let err = st.and_then(|j| j.last_error.clone());
                let control = controls
                    .iter()
                    .find(|(c, _)| *c == n)
                    .map(|(_, c)| c.clone())
                    .unwrap_or_default();
                let (state, pill) = if control.paused {
                    (
                        format!(
                            "paused{}",
                            control
                                .paused_by
                                .as_deref()
                                .map(|b| format!(" by {b}"))
                                .unwrap_or_default()
                        ),
                        "amber",
                    )
                } else if control.run_requested.is_some() {
                    ("run asked for".to_owned(), "on")
                } else if err.is_some() {
                    ("failing".to_owned(), "red")
                } else {
                    ("ok".to_owned(), "fine")
                };
                let mut row = vec![
                    Cell::new("name", n.clone()),
                    Cell::new("name", holder),
                    Cell::new(
                        "when",
                        st.and_then(|j| j.last_run_at)
                            .map_or_else(|| "never".into(), when),
                    ),
                    Cell::new(
                        "when",
                        st.and_then(|j| j.last_ok_at)
                            .map_or_else(|| "never".into(), when),
                    ),
                    Cell::new("num", st.map_or(0, |j| j.runs).to_string()),
                    Cell::new("state", state).pill(pill),
                    Cell::new("text", err.unwrap_or_default()),
                ];
                if can_run {
                    if control.paused {
                        row.push(Cell::new("actions", ""));
                        row.push(button(
                            "/admin/jobs/pause",
                            "Resume",
                            &[("name", &n), ("paused", "false")],
                            false,
                        ));
                    } else {
                        row.push(button("/admin/jobs/run", "Run now", &[("name", &n)], false));
                        row.push(button(
                            "/admin/jobs/pause",
                            "Pause",
                            &[("name", &n), ("paused", "true")],
                            true,
                        ));
                    }
                }
                row
            })
            .collect(),
        empty: "No job has run yet.".into(),
    });
    let nodes = s
        .store
        .live_slot_holders(locus_store::env::CLOCK_SLOT_FAMILY)
        .await
        .unwrap_or_default();
    p.tables.push(Table {
        title: "Nodes".into(),
        cols: cols(&[("name", "Node")]),
        rows: nodes
            .into_iter()
            .map(|n| vec![Cell::new("name", n)])
            .collect(),
        empty: "None.".into(),
    });
    p
}

/// `POST /admin/jobs/run` and `/admin/jobs/pause` (admins): through
/// `admin.jobs.runNow` and `admin.jobs.setPaused`.
pub async fn job_action(
    State(s): St,
    headers: HeaderMap,
    axum::extract::Path(action): axum::extract::Path<String>,
    body: Bytes,
) -> Response {
    let (op, form) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let name = web::field(&form, "name").to_owned();
    let paused = web::field(&form, "paused") == "true";
    let args = Json(input(json!({"name": name, "paused": paused})));
    let (result, done) = match action.as_str() {
        "run" => (
            api(
                crate::locus_admin::run_job_now(State(Arc::clone(&s)), op.api_headers(), args)
                    .await,
            ),
            format!("{name} runs now."),
        ),
        "pause" => (
            api(
                crate::locus_admin::set_job_paused(State(Arc::clone(&s)), op.api_headers(), args)
                    .await,
            ),
            if paused {
                format!("{name} is paused.")
            } else {
                format!("{name} is resumed.")
            },
        ),
        _ => return (StatusCode::NOT_FOUND, "Not found").into_response(),
    };
    match result {
        Ok(_) => web::redirect(&format!(
            "/admin/jobs?done={}",
            url::form_urlencoded::byte_serialize(done.as_bytes()).collect::<String>()
        )),
        Err(e) => {
            let p = jobs_page(&s, &op, None, vec![e]).await;
            web::render(&p, StatusCode::BAD_REQUEST)
        }
    }
}

// ---------------------------------------------------------------- shards

/// `GET /admin/shards`: in the entryway role, the shards behind it, their
/// queued callbacks, and a token-key rotation's progress. (Added, weighted
/// and drained with `aurora-locus shard`.)
pub async fn shards(State(s): St, headers: HeaderMap) -> Response {
    let op = op_or_return!(s, headers);
    let entryway = s.role == locus_core::bootstrap::Role::Entryway;
    let mut p = section(
        &s,
        &op,
        PageId::Shards,
        if entryway {
            "The PDSes behind this entryway. Add, weight and drain them with `aurora-locus shard`."
        } else {
            "Shards are for the entryway role; this node isn't one."
        },
    )
    .await;
    if !entryway {
        return web::render(&p, StatusCode::OK);
    }
    let backlog = s.store.shard_backlog().await.unwrap_or_default();
    let shards = s.store.shards().await.unwrap_or_default();
    p.tables.push(Table {
        title: "Shards".into(),
        cols: cols(&[
            ("name", "Name"),
            ("text", "URL"),
            ("num", "Accounts"),
            ("num", "Weight"),
            ("state", "Kind"),
            ("state", "Placement"),
            ("num", "Queued callbacks"),
            ("text", "Last error"),
        ]),
        rows: shards
            .iter()
            .map(|sh| {
                let (queued, err) = backlog
                    .iter()
                    .find(|(n, _, _)| *n == sh.name)
                    .map_or((0, None), |(_, q, e)| (*q, e.clone()));
                vec![
                    Cell::new("name", sh.name.clone()),
                    Cell::new("text", sh.url.clone()),
                    Cell::new("num", sh.accounts.to_string()),
                    Cell::new("num", sh.weight.to_string()),
                    Cell::new("state", if sh.locus { "locus" } else { "non-locus" })
                        .pill(if sh.locus { "fine" } else { "amber" }),
                    Cell::new(
                        "state",
                        if sh.draining {
                            "draining"
                        } else {
                            "taking accounts"
                        },
                    )
                    .pill(if sh.draining { "amber" } else { "fine" }),
                    Cell::new("num", queued.to_string()),
                    Cell::new("text", err.unwrap_or_default()),
                ]
            })
            .collect(),
        empty: "No shard yet: `aurora-locus shard add <url> --admin-password-file <file>`.".into(),
    });
    if let Ok(Some(r)) = s.store.key_rotation().await
        && r.finished_at.is_none()
    {
        let acked = r.acknowledged_shards();
        p.tables.push(Table {
            title: "Token key rotation".into(),
            cols: cols(&[("name", "Shard"), ("state", "Has the new key")]),
            rows: shards
                .iter()
                .map(|sh| {
                    let ok = acked.contains(&sh.name) || r.switched_at.is_some();
                    vec![
                        Cell::new("name", sh.name.clone()),
                        Cell::new(
                            "state",
                            match (ok, sh.locus) {
                                (true, _) => "yes",
                                (false, true) => "being sent",
                                (false, false) => "update it, then --confirm",
                            },
                        )
                        .pill(if ok { "fine" } else { "amber" }),
                    ]
                })
                .collect(),
            empty: "None.".into(),
        });
    }
    web::render(&p, StatusCode::OK)
}

// ---------------------------------------------------------------- audit log

/// `GET /admin/audit?action=&actor=&before=`.
pub async fn audit(State(s): St, headers: HeaderMap, Query(q): Query<PageQuery>) -> Response {
    let op = op_or_return!(s, headers);
    let mut p = section(
        &s,
        &op,
        PageId::Audit,
        "Every operator action and setting change, newest first.",
    )
    .await;
    let action = q.action.unwrap_or_default();
    let actor = q.actor.unwrap_or_default();
    p.forms.push(Form {
        title: "Filter".into(),
        text: String::new(),
        action: "/admin/audit".into(),
        get: true,
        inputs: vec![
            Input::new("action", "Action contains", "text").value(action.clone()),
            Input::new("actor", "By (contains)", "text").value(actor.clone()),
        ],
        hidden: Vec::new(),
        button: "Filter".into(),
        danger: false,
    });
    let entries = s
        .store
        .audit_entries(q.before, 1000)
        .await
        .unwrap_or_default();
    let last_id = entries.last().map(|e| e.id);
    let shown: Vec<_> = entries
        .into_iter()
        .filter(|e| e.action.contains(&action) && e.actor.contains(&actor))
        .take(200)
        .collect();
    let next = last_id
        .filter(|_| shown.len() == 200)
        .map(|id| id.to_string());
    p.tables.push(Table {
        title: "Entries".into(),
        cols: cols(&[
            ("when", "When"),
            ("name", "By"),
            ("name", "Action"),
            ("text", "On"),
            ("text", "Detail"),
        ]),
        rows: shown
            .into_iter()
            .map(|e| {
                vec![
                    Cell::new("when", when(e.at)),
                    Cell::new("name", e.actor),
                    Cell::new("name", e.action),
                    Cell::new("text", e.target.unwrap_or_default()),
                    Cell::new("text", e.detail_json.unwrap_or_default()).code(),
                ]
            })
            .collect(),
        empty: "Nothing matches.".into(),
    });
    p.forms.push(Form {
        title: "Export".into(),
        text: "The whole log as JSON lines, newest first.".into(),
        action: "/admin/audit/export".into(),
        get: true,
        inputs: Vec::new(),
        hidden: next
            .map(|n| vec![("before".to_owned(), n)])
            .unwrap_or_default(),
        button: "Download".into(),
        danger: false,
    });
    web::render(&p, StatusCode::OK)
}

/// `GET /admin/audit/export`: JSON lines.
pub async fn audit_export(State(s): St, headers: HeaderMap) -> Response {
    let op = op_or_return!(s, headers);
    let _ = &op;
    let mut out = String::new();
    let mut before = None;
    loop {
        let page = s
            .store
            .audit_entries(before, 1000)
            .await
            .unwrap_or_default();
        if page.is_empty() {
            break;
        }
        for e in &page {
            out.push_str(&json!({
                "id": e.id, "at": locus_core::time::iso8601(e.at), "actor": e.actor, "action": e.action,
                "target": e.target, "detail": e.detail_json.as_deref().and_then(|d| serde_json::from_str::<Value>(d).ok()),
            }).to_string());
            out.push('\n');
        }
        before = page.last().map(|e| e.id);
        if page.len() < 1000 {
            break;
        }
    }
    let mut res = out.into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-ndjson"),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"audit-log.jsonl\""),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

// ---------------------------------------------------------------- logs

/// `GET /admin/logs` (admins): this node's recent log lines.
pub async fn logs(State(s): St, headers: HeaderMap) -> Response {
    let op = op_or_return!(s, headers);
    if op.role < Role::Admin {
        return (StatusCode::FORBIDDEN, "Logs need the admin role.").into_response();
    }
    let mut p = section(
        &s,
        &op,
        PageId::Logs,
        "The newest lines this node logged, oldest first. Other nodes keep their own.",
    )
    .await;
    let lines = s.logs.recent(500);
    p.pre = Some((
        format!("{} line(s) on {}", lines.len(), s.node_id),
        lines.join("\n"),
    ));
    p.fields = fields(&s, &["log.level"]);
    web::render(&p, StatusCode::OK)
}
