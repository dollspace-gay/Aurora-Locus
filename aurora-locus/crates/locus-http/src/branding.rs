//! Branding: the published theme (cached, re-read every few seconds so
//! every node follows a publish), its uploads and stylesheets, the
//! console's editor with previews, and dressing pages with it.
//!
//! The OAuth sign-in and consent pages get only the checked variables
//! stylesheet; the portal and sign-up pages get the full one (custom CSS,
//! an uploaded font).

#![allow(clippy::result_large_err)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use locus_store::branding::AssetRow;
use locus_web::branding::{self as brand, Mode, Theme};
use locus_web::console::PageId;
use locus_web::console_pages::{Card, Cell, Fact, Form, Frame, Input, SectionPage, Table};
use locus_web::setup::Choice;
use locus_web::view::{Dress, Link};
use serde::Deserialize;
use serde_json::json;

use crate::admin_auth::Role;
use crate::console::{Op, form_op, require_op, shell, when};
use crate::state::AppState;
use crate::web;

type St = State<Arc<AppState>>;

/// The upload form's path (its body limit is its own).
pub const UPLOAD_PATH: &str = "/admin/branding/upload";
/// The largest upload request.
pub const MAX_UPLOAD_BODY: i64 = 4 * 1024 * 1024;
/// How long an unused upload is kept.
const ASSET_GRACE_MS: i64 = 7 * 86_400_000;
const REFRESH: Duration = Duration::from_secs(5);

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/branding/{id}", get(asset))
        .route("/branding/theme/{version}/{kind}", get(theme_css))
        .route("/admin/branding", get(editor))
        .route("/admin/branding/save", post(save))
        .route("/admin/branding/publish", post(publish))
        .route("/admin/branding/rollback", post(rollback))
        .route("/admin/branding/off", post(off))
        .route(UPLOAD_PATH, post(upload))
        .route("/admin/branding/draft.css", get(draft_css))
        .route("/admin/branding/preview/{page}", get(preview))
}

// ---------------------------------------------------------------- the cache

/// The theme in use.
pub struct Published {
    pub version: i64,
    pub theme: Theme,
}

#[derive(Default)]
pub struct Cache(Mutex<Option<(Instant, Option<Arc<Published>>)>>);

impl Cache {
    /// Forgets the cached theme (after a publish on this node).
    pub fn invalidate(&self) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

/// The theme in use, if any.
pub async fn published(s: &AppState) -> Option<Arc<Published>> {
    {
        let c = s
            .branding
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((at, p)) = c.as_ref()
            && at.elapsed() < REFRESH
        {
            return p.clone();
        }
    }
    let fresh = match s.store.branding_active().await {
        Ok(Some(row)) => serde_json::from_str::<Theme>(&row.theme_json)
            .ok()
            .map(|theme| {
                Arc::new(Published {
                    version: row.version,
                    theme,
                })
            }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!("branding read failed: {e}");
            return None;
        }
    };
    *s.branding
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((Instant::now(), fresh.clone()));
    fresh
}

fn theme_href(version: i64, full: bool) -> String {
    format!(
        "/branding/theme/{version}/{}",
        if full { "full.css" } else { "vars.css" }
    )
}

fn links(t: &Theme) -> Vec<Link> {
    [
        (&t.links.home, "Home"),
        (&t.links.terms, "Terms of service"),
        (&t.links.privacy, "Privacy policy"),
        (&t.links.support, "Support"),
    ]
    .into_iter()
    .filter(|(h, _)| !h.is_empty())
    .map(|(h, l)| Link {
        href: h.clone(),
        label: l.into(),
    })
    .collect()
}

/// Which page text a dressed page shows under its title.
#[derive(Debug, Clone, Copy)]
pub enum Blurb {
    None,
    SignIn,
    SignUp,
    Portal,
}

fn dress_for(t: &Theme, css: String, blurb: Blurb) -> Dress {
    // The favicon, or the logo when there's none.
    let icon = if t.favicon.is_empty() {
        &t.logo
    } else {
        &t.favicon
    };
    Dress {
        css,
        favicon: if icon.is_empty() {
            String::new()
        } else {
            brand::asset_path(icon)
        },
        logo: if t.logo.is_empty() {
            String::new()
        } else {
            brand::asset_path(&t.logo)
        },
        links: links(t),
        footer: t.text.footer.clone(),
        blurb: match blurb {
            Blurb::None => String::new(),
            Blurb::SignIn => t.text.sign_in_blurb.clone(),
            Blurb::SignUp => t.text.sign_up_blurb.clone(),
            Blurb::Portal => t.text.portal_blurb.clone(),
        },
        html_class: String::new(),
    }
}

/// Branding for an account-portal or sign-up page (the full stylesheet).
pub async fn dress(s: &AppState, blurb: Blurb) -> Option<Dress> {
    let p = published(s).await?;
    Some(dress_for(&p.theme, theme_href(p.version, true), blurb))
}

/// The service name a theme sets, or the setting's (or the hostname).
pub async fn service_name(s: &AppState) -> String {
    match published(s).await {
        Some(p) if !p.theme.service_name.is_empty() => p.theme.service_name.clone(),
        _ => crate::console::brand(s),
    }
}

/// What the OAuth pages take from the theme: the checked variables (or,
/// for sign-up, everything), the logo and the links.
#[derive(Debug, Clone, Default)]
pub struct OauthBrand {
    pub css: String,
    pub logo: String,
    pub links: Vec<Link>,
}

pub async fn oauth_brand(s: &AppState, full: bool) -> OauthBrand {
    match published(s).await {
        Some(p) => OauthBrand {
            css: theme_href(p.version, full),
            logo: if p.theme.logo.is_empty() {
                String::new()
            } else {
                brand::asset_path(&p.theme.logo)
            },
            links: links(&p.theme),
        },
        None => OauthBrand::default(),
    }
}

/// The footer line added to emails.
pub async fn email_footer(s: &AppState) -> Option<String> {
    published(s)
        .await
        .map(|p| p.theme.text.footer.clone())
        .filter(|f| !f.is_empty())
}

// ---------------------------------------------------------------- serving

/// `GET /branding/{id}`: an upload, as stored.
pub async fn asset(State(s): St, Path(id): Path<String>) -> Response {
    let Ok(Some(row)) = s.store.branding_asset(&id).await else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };
    let reader = match s.blobs.read(&row.storage_key, None).await {
        Ok(Some(r)) => r,
        _ => return (StatusCode::NOT_FOUND, "Not found").into_response(),
    };
    let body = axum::body::Body::from_stream(reader.body);
    let mut res = body.into_response();
    let h = res.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&row.mime) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    res
}

fn css_response(css: String, cache: &'static str) -> Response {
    let mut res = css.into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/css; charset=utf-8"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    res
}

/// `GET /branding/theme/{version}/{vars.css|full.css}`: a published
/// version's stylesheet (versions never change, so it's cached for good).
pub async fn theme_css(State(s): St, Path((version, kind)): Path<(i64, String)>) -> Response {
    let full = match kind.as_str() {
        "vars.css" => false,
        "full.css" => true,
        _ => return (StatusCode::NOT_FOUND, "Not found").into_response(),
    };
    let row = match s.store.branding_version(version).await {
        Ok(Some(r)) if r.state == "published" => r,
        _ => return (StatusCode::NOT_FOUND, "Not found").into_response(),
    };
    let Ok(t) = serde_json::from_str::<Theme>(&row.theme_json) else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };
    let css = if full {
        brand::full_css(&t, Mode::Auto)
    } else {
        brand::vars_css(&t, Mode::Auto)
    };
    css_response(css, "public, max-age=31536000, immutable")
}

// ---------------------------------------------------------------- the editor

async fn assets_list(s: &AppState) -> Vec<AssetRow> {
    s.store.branding_assets().await.unwrap_or_default()
}

fn asset_pairs(a: &[AssetRow]) -> Vec<(String, String)> {
    a.iter().map(|r| (r.id.clone(), r.kind.clone())).collect()
}

/// The draft, or (with none) the theme in use, or the built-in one.
async fn working_theme(s: &AppState) -> Theme {
    if let Ok(Some(d)) = s.store.branding_draft().await
        && let Ok(t) = serde_json::from_str(&d.theme_json)
    {
        return t;
    }
    if let Ok(Some(a)) = s.store.branding_active().await
        && let Ok(t) = serde_json::from_str(&a.theme_json)
    {
        return t;
    }
    Theme::default()
}

/// The theme as previews and publishing see it: custom CSS sanitized (or
/// dropped while it doesn't pass).
fn cleaned(t: &Theme, assets: &[AssetRow]) -> Theme {
    let paths: Vec<String> = assets.iter().map(|a| brand::asset_path(&a.id)).collect();
    Theme {
        custom_css: brand::sanitize_css(&t.custom_css, &paths).unwrap_or_default(),
        ..t.clone()
    }
}

fn choice(value: &str, label: &str, current: &str) -> Choice {
    Choice {
        value: value.into(),
        label: label.into(),
        checked: value == current,
    }
}

fn pick(name: &str, label: &str, options: Vec<Choice>) -> Input {
    Input {
        options,
        ..Input::new(name, label, "select")
    }
}

fn asset_pick(
    name: &str,
    label: &str,
    assets: &[AssetRow],
    kinds: &[&str],
    current: &str,
) -> Input {
    let mut opts = vec![choice("", "None", current)];
    opts.extend(
        assets
            .iter()
            .filter(|a| kinds.contains(&a.kind.as_str()))
            .map(|a| choice(&a.id, &format!("{} ({})", a.name, a.kind), current)),
    );
    pick(name, label, opts)
}

async fn editor_render(
    s: &AppState,
    op: &Op,
    notice: Option<String>,
    mut errors: Vec<String>,
    status: StatusCode,
) -> Response {
    let assets = assets_list(s).await;
    let t = working_theme(s).await;
    let active = s.store.branding_active().await.ok().flatten();
    let draft = s.store.branding_draft().await.ok().flatten();
    let mut p = SectionPage {
        shell: shell(s, op, PageId::Branding).await,
        csrf: op.csrf.clone(),
        intro: "How the pages account holders see look: the account portal, sign-up, and the OAuth sign-in and consent pages. Changes are a draft until you publish them. The sign-in and consent pages take only the colours, logo, backgrounds, a bundled font and the presets, never custom CSS, so a theme can't disguise what someone approves.".into(),
        notice,
        errors: Vec::new(),
        cards: Vec::new(),
        forms: Vec::new(),
        tables: Vec::new(),
        fields: Vec::new(),
        advanced: Vec::new(),
        pre: None,
        frames: Vec::new(),
        can_edit: op.role >= Role::Admin,
        back: PageId::Branding.href().into(),
    };
    let problems = t.problems(&asset_pairs(&assets));
    p.cards.push(Card {
        title: "State".into(),
        facts: vec![
            Fact::new(
                "In use",
                active.as_ref().map_or_else(
                    || "the built-in look".into(),
                    |a| format!("version {}", a.version),
                ),
            ),
            Fact::new(
                "Draft",
                draft.as_ref().map_or_else(
                    || "none (editing starts from the version in use)".into(),
                    |d| format!("saved {} by {}", when(d.created_at), d.created_by),
                ),
            ),
            Fact::new(
                "Ready to publish",
                if problems.is_empty() {
                    "yes"
                } else {
                    "not yet"
                },
            )
            .class(if problems.is_empty() { "ok" } else { "warn" }),
        ],
    });
    errors.extend(problems);
    p.errors = errors;
    let c = |mode: &str, name: &str, v: &str| {
        Input::new(
            &format!("{mode}_{name}"),
            &format!("{} {name}", if mode == "light" { "Light" } else { "Dark" }),
            "text",
        )
        .value(v)
        .hint("#rrggbb, or empty for the built-in colour")
    };
    let mut inputs = vec![
        Input::new("service_name", "Service name", "text")
            .value(t.service_name.clone())
            .hint("Empty keeps the service name setting."),
        asset_pick("logo", "Logo", &assets, &["logo"], &t.logo),
        asset_pick(
            "favicon",
            "Favicon",
            &assets,
            &["favicon", "logo"],
            &t.favicon,
        ),
        asset_pick(
            "background_light",
            "Light background image",
            &assets,
            &["background"],
            &t.background_light,
        ),
        asset_pick(
            "background_dark",
            "Dark background image",
            &assets,
            &["background"],
            &t.background_dark,
        ),
    ];
    for (mode, colors) in [("light", &t.light), ("dark", &t.dark)] {
        inputs.extend([
            c(mode, "primary", &colors.primary),
            c(mode, "error", &colors.error),
            c(mode, "warning", &colors.warning),
            c(mode, "info", &colors.info),
            c(mode, "success", &colors.success),
        ]);
    }
    inputs.extend([
        pick(
            "font",
            "Font",
            vec![
                choice("system", "The device's fonts", &t.font),
                choice("hyperlegible", "Atkinson Hyperlegible", &t.font),
                choice("plex", "IBM Plex Sans", &t.font),
            ],
        ),
        asset_pick(
            "uploaded_font",
            "Uploaded font (portal and sign-up only)",
            &assets,
            &["font"],
            &t.uploaded_font,
        ),
        pick(
            "radius",
            "Corners",
            vec![
                choice("sharp", "Sharp", &t.radius),
                choice("default", "Rounded", &t.radius),
                choice("round", "Very round", &t.radius),
            ],
        ),
        pick(
            "spacing",
            "Spacing",
            vec![
                choice("compact", "Compact", &t.spacing),
                choice("default", "Normal", &t.spacing),
                choice("roomy", "Roomy", &t.spacing),
            ],
        ),
        Input::new("link_home", "Home link", "text").value(t.links.home.clone()),
        Input::new("link_terms", "Terms of service link", "text").value(t.links.terms.clone()),
        Input::new("link_privacy", "Privacy policy link", "text").value(t.links.privacy.clone()),
        Input::new("link_support", "Support link (https:// or mailto:)", "text")
            .value(t.links.support.clone()),
        Input::new("text_sign_in_heading", "Sign-in heading (portal)", "text")
            .value(t.text.sign_in_heading.clone()),
        Input::new("text_sign_in_blurb", "Sign-in text (portal)", "textarea")
            .value(t.text.sign_in_blurb.clone()),
        Input::new("text_sign_up_blurb", "Sign-up text", "textarea")
            .value(t.text.sign_up_blurb.clone()),
        Input::new("text_portal_blurb", "Portal overview text", "textarea")
            .value(t.text.portal_blurb.clone()),
        Input::new(
            "text_footer",
            "Footer (portal, sign-up and emails)",
            "textarea",
        )
        .value(t.text.footer.clone()),
        Input::new(
            "custom_css",
            "Custom CSS (portal and sign-up only)",
            "textarea",
        )
        .value(t.custom_css.clone())
        .hint("Rules and @media only; url() may name an upload by its path, /branding/<id>."),
    ]);
    if op.role >= Role::Admin {
        p.forms.push(Form {
            title: "Draft".into(),
            text: String::new(),
            action: "/admin/branding/save".into(),
            get: false,
            inputs,
            hidden: Vec::new(),
            button: "Save the draft".into(),
            danger: false,
        });
        p.forms.push(Form {
            title: "Publish".into(),
            text: "The draft becomes a new version and is used at once on every node.".into(),
            action: "/admin/branding/publish".into(),
            get: false,
            inputs: Vec::new(),
            hidden: Vec::new(),
            button: "Publish the draft".into(),
            danger: false,
        });
        p.forms.push(Form {
            title: "Upload".into(),
            text: "PNG, JPEG, GIF, WebP or ICO images; WOFF2, WOFF, TTF or OTF fonts. Logos and favicons up to 1 MB, backgrounds 3 MB, fonts 2 MB.".into(),
            action: UPLOAD_PATH.into(),
            get: false,
            inputs: vec![
                pick("kind", "It's a", vec![choice("logo", "Logo", "logo"), choice("favicon", "Favicon", ""), choice("background", "Background", ""), choice("font", "Font", "")]),
                Input::new("file", "File", "file"),
            ],
            hidden: Vec::new(),
            button: "Upload".into(),
            danger: false,
        });
        if active.is_some() {
            p.forms.push(Form {
                title: "Back to the built-in look".into(),
                text: "Stops using a theme. Published versions are kept, so you can put one back."
                    .into(),
                action: "/admin/branding/off".into(),
                get: false,
                inputs: Vec::new(),
                hidden: Vec::new(),
                button: "Stop using the theme".into(),
                danger: true,
            });
        }
    }
    let versions = s.store.branding_versions(50).await.unwrap_or_default();
    let in_use = active.as_ref().map(|a| a.version);
    p.tables.push(Table {
        title: "Published versions".into(),
        cols: vec![
            ("num", "Version".into()),
            ("when", "Published".into()),
            ("name", "By".into()),
            ("state", "".into()),
            ("actions", "".into()),
        ],
        rows: versions
            .into_iter()
            .map(|v| {
                let n = v.version.to_string();
                let mut row = vec![
                    Cell::new("num", n.clone()),
                    Cell::new("when", v.published_at.map(when).unwrap_or_default()),
                    Cell::new("name", v.published_by.unwrap_or_default()),
                    if Some(v.version) == in_use {
                        Cell::new("state", "in use").pill("on")
                    } else {
                        Cell::new("state", "")
                    },
                ];
                if Some(v.version) != in_use && op.role >= Role::Admin {
                    row.push(Cell {
                        class: "actions",
                        button: Some(locus_web::console_pages::Button {
                            action: "/admin/branding/rollback".into(),
                            label: "Use this version".into(),
                            hidden: vec![("version".into(), n)],
                            danger: false,
                        }),
                        ..Cell::default()
                    });
                } else {
                    row.push(Cell::new("actions", ""));
                }
                row
            })
            .collect(),
        empty: "None yet.".into(),
    });
    p.tables.push(Table {
        title: "Uploads".into(),
        cols: vec![
            ("name", "Name".into()),
            ("state", "Kind".into()),
            ("num", "Size".into()),
            ("text", "Path".into()),
            ("state", "".into()),
        ],
        rows: assets
            .iter()
            .map(|a| {
                vec![
                    Cell::new("name", a.name.clone()),
                    Cell::new("state", a.kind.clone()),
                    Cell::new("num", crate::console::bytes(a.size)),
                    Cell::new("text", brand::asset_path(&a.id)).code(),
                    if a.unused_since.is_some() {
                        Cell::new("state", "unused").pill("amber")
                    } else {
                        Cell::new("state", "used").pill("on")
                    },
                ]
            })
            .collect(),
        empty: "Nothing uploaded yet.".into(),
    });
    for (page, label) in [
        ("sign-in", "OAuth sign-in"),
        ("consent", "OAuth consent"),
        ("sign-up", "Sign-up"),
        ("portal", "Account portal"),
    ] {
        for mode in ["light", "dark"] {
            for (width, device) in [(375, "phone"), (1100, "desktop")] {
                p.frames.push(Frame {
                    title: format!("{label}, {mode}, {device}"),
                    src: format!("/admin/branding/preview/{page}?mode={mode}"),
                    width,
                });
            }
        }
    }
    web::page_csp(
        match askama::Template::render(&p) {
            Ok(h) => h,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        },
        status,
        &format!("{}; frame-src 'self'", locus_web::assets::CSP),
    )
}

#[derive(Deserialize, Default)]
pub struct DoneQuery {
    #[serde(default)]
    done: Option<String>,
}

/// `GET /admin/branding`.
pub async fn editor(State(s): St, headers: HeaderMap, Query(q): Query<DoneQuery>) -> Response {
    let op = match require_op(&s, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let notice = q.done.map(|d| match d.as_str() {
        "saved" => "Draft saved.".to_owned(),
        "published" => "Published: the new version is in use.".to_owned(),
        "rollback" => "That version is in use again.".to_owned(),
        "off" => "The built-in look is in use.".to_owned(),
        "uploaded" => "Uploaded. Pick it in the draft.".to_owned(),
        _ => "Done.".to_owned(),
    });
    editor_render(&s, &op, notice, Vec::new(), StatusCode::OK).await
}

async fn admin_form(
    s: &AppState,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<(Op, Vec<(String, String)>), Response> {
    let (op, form) = form_op(s, headers, body).await?;
    if op.role < Role::Admin {
        return Err(editor_render(
            s,
            &op,
            None,
            vec!["Branding needs the admin role.".into()],
            StatusCode::FORBIDDEN,
        )
        .await);
    }
    Ok((op, form))
}

/// The theme the draft form describes.
fn theme_from(form: &[(String, String)]) -> Theme {
    let f = |k: &str| web::field(form, k).to_owned();
    let raw = |k: &str| {
        form.iter()
            .find(|(n, _)| n == k)
            .map_or(String::new(), |(_, v)| v.replace("\r\n", "\n"))
    };
    let colors = |m: &str| brand::Colors {
        primary: f(&format!("{m}_primary")).to_ascii_lowercase(),
        error: f(&format!("{m}_error")).to_ascii_lowercase(),
        warning: f(&format!("{m}_warning")).to_ascii_lowercase(),
        info: f(&format!("{m}_info")).to_ascii_lowercase(),
        success: f(&format!("{m}_success")).to_ascii_lowercase(),
    };
    Theme {
        service_name: f("service_name"),
        logo: f("logo"),
        favicon: f("favicon"),
        background_light: f("background_light"),
        background_dark: f("background_dark"),
        light: colors("light"),
        dark: colors("dark"),
        links: brand::Links {
            home: f("link_home"),
            terms: f("link_terms"),
            privacy: f("link_privacy"),
            support: f("link_support"),
        },
        font: f("font"),
        uploaded_font: f("uploaded_font"),
        radius: f("radius"),
        spacing: f("spacing"),
        text: brand::Texts {
            sign_in_heading: f("text_sign_in_heading"),
            sign_in_blurb: raw("text_sign_in_blurb").trim().to_owned(),
            sign_up_blurb: raw("text_sign_up_blurb").trim().to_owned(),
            portal_blurb: raw("text_portal_blurb").trim().to_owned(),
            footer: raw("text_footer").trim().to_owned(),
        },
        custom_css: raw("custom_css"),
    }
}

/// `POST /admin/branding/save`: the draft, saved even with problems (they
/// block publishing, not saving).
pub async fn save(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (op, form) = match admin_form(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let t = theme_from(&form);
    let json = serde_json::to_string(&t).unwrap_or_default();
    match s
        .store
        .save_branding_draft(&json, &t.assets(), &op.actor())
        .await
    {
        Ok(_) => web::redirect("/admin/branding?done=saved"),
        Err(e) => {
            editor_render(
                &s,
                &op,
                None,
                vec![e.to_string()],
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .await
        }
    }
}

/// `POST /admin/branding/publish`: only a draft with no problems; its
/// custom CSS is stored as sanitized.
pub async fn publish(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (op, _) = match admin_form(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let assets = assets_list(&s).await;
    let Ok(Some(draft)) = s.store.branding_draft().await else {
        return editor_render(
            &s,
            &op,
            None,
            vec!["There's no draft to publish: save one first.".into()],
            StatusCode::BAD_REQUEST,
        )
        .await;
    };
    let Ok(t) = serde_json::from_str::<Theme>(&draft.theme_json) else {
        return editor_render(
            &s,
            &op,
            None,
            vec!["The draft can't be read.".into()],
            StatusCode::BAD_REQUEST,
        )
        .await;
    };
    let problems = t.problems(&asset_pairs(&assets));
    if !problems.is_empty() {
        return editor_render(&s, &op, None, problems, StatusCode::BAD_REQUEST).await;
    }
    let clean = cleaned(&t, &assets);
    let result = async {
        s.store
            .save_branding_draft(
                &serde_json::to_string(&clean).unwrap_or_default(),
                &clean.assets(),
                &op.actor(),
            )
            .await?;
        s.store.publish_branding_draft(&op.actor()).await
    }
    .await;
    match result {
        Ok(Some(v)) => {
            s.branding.invalidate();
            let _ = s
                .store
                .audit(&op.actor(), "branding.publish", Some(&v.to_string()), None)
                .await;
            web::redirect("/admin/branding?done=published")
        }
        Ok(None) => {
            editor_render(
                &s,
                &op,
                None,
                vec!["There's no draft to publish.".into()],
                StatusCode::BAD_REQUEST,
            )
            .await
        }
        Err(e) => {
            editor_render(
                &s,
                &op,
                None,
                vec![e.to_string()],
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .await
        }
    }
}

/// `POST /admin/branding/rollback`: a published version back in use.
pub async fn rollback(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (op, form) = match admin_form(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let v: i64 = web::field(&form, "version").parse().unwrap_or(-1);
    match s.store.activate_branding(v).await {
        Ok(true) => {
            s.branding.invalidate();
            let _ = s
                .store
                .audit(&op.actor(), "branding.rollback", Some(&v.to_string()), None)
                .await;
            web::redirect("/admin/branding?done=rollback")
        }
        Ok(false) => {
            editor_render(
                &s,
                &op,
                None,
                vec!["No such published version.".into()],
                StatusCode::BAD_REQUEST,
            )
            .await
        }
        Err(e) => {
            editor_render(
                &s,
                &op,
                None,
                vec![e.to_string()],
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .await
        }
    }
}

/// `POST /admin/branding/off`.
pub async fn off(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (op, _) = match admin_form(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let _ = s.store.deactivate_branding().await;
    s.branding.invalidate();
    let _ = s.store.audit(&op.actor(), "branding.off", None, None).await;
    web::redirect("/admin/branding?done=off")
}

/// What a file is, from its first bytes: its type, and the kinds of upload
/// it may be.
#[must_use]
pub fn sniff(head: &[u8]) -> Option<(&'static str, &'static [&'static str])> {
    const IMAGE: &[&str] = &["logo", "favicon", "background"];
    const FONT: &[&str] = &["font"];
    let starts = |m: &[u8]| head.starts_with(m);
    Some(if starts(b"\x89PNG\r\n\x1a\n") {
        ("image/png", IMAGE)
    } else if starts(b"\xff\xd8\xff") {
        ("image/jpeg", IMAGE)
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        ("image/gif", IMAGE)
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        ("image/webp", IMAGE)
    } else if starts(b"\x00\x00\x01\x00") {
        ("image/x-icon", &["favicon"])
    } else if starts(b"wOF2") {
        ("font/woff2", FONT)
    } else if starts(b"wOFF") {
        ("font/woff", FONT)
    } else if starts(b"\x00\x01\x00\x00") {
        ("font/ttf", FONT)
    } else if starts(b"OTTO") {
        ("font/otf", FONT)
    } else {
        return None;
    })
}

fn max_size(kind: &str) -> usize {
    match kind {
        "background" => 3 * 1024 * 1024,
        "font" => 2 * 1024 * 1024,
        _ => 1024 * 1024,
    }
}

/// `POST /admin/branding/upload` (multipart: csrf, kind, file).
pub async fn upload(State(s): St, headers: HeaderMap, mut mp: Multipart) -> Response {
    let op = match require_op(&s, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let (mut csrf, mut kind, mut file, mut name) =
        (String::new(), String::new(), None::<Bytes>, String::new());
    loop {
        match mp.next_field().await {
            Ok(Some(f)) => {
                let field = f.name().unwrap_or_default().to_owned();
                if field == "file" {
                    name = f
                        .file_name()
                        .unwrap_or("upload")
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(100)
                        .collect();
                }
                match f.bytes().await {
                    Ok(b) => match field.as_str() {
                        "csrf" => csrf = String::from_utf8_lossy(&b).into_owned(),
                        "kind" => kind = String::from_utf8_lossy(&b).into_owned(),
                        "file" => file = Some(b),
                        _ => {}
                    },
                    Err(e) => {
                        return editor_render(
                            &s,
                            &op,
                            None,
                            vec![format!("The upload didn't arrive whole: {e}")],
                            StatusCode::BAD_REQUEST,
                        )
                        .await;
                    }
                }
            }
            Ok(None) => break,
            Err(e) => {
                return editor_render(
                    &s,
                    &op,
                    None,
                    vec![format!("The upload didn't arrive whole: {e}")],
                    StatusCode::BAD_REQUEST,
                )
                .await;
            }
        }
    }
    if !web::same_site(&headers) || !web::ct_eq(&csrf, &op.csrf) {
        return (
            StatusCode::FORBIDDEN,
            "Request refused: the form expired. Go back and try again.",
        )
            .into_response();
    }
    if op.role < Role::Admin {
        return editor_render(
            &s,
            &op,
            None,
            vec!["Branding needs the admin role.".into()],
            StatusCode::FORBIDDEN,
        )
        .await;
    }
    let Some(bytes) = file.filter(|b| !b.is_empty()) else {
        return editor_render(
            &s,
            &op,
            None,
            vec!["Choose a file.".into()],
            StatusCode::BAD_REQUEST,
        )
        .await;
    };
    let Some((mime, kinds)) = sniff(&bytes) else {
        return editor_render(&s, &op, None, vec!["That isn't a PNG, JPEG, GIF, WebP or ICO image, or a WOFF2, WOFF, TTF or OTF font.".into()], StatusCode::BAD_REQUEST).await;
    };
    if !kinds.contains(&kind.as_str()) {
        return editor_render(
            &s,
            &op,
            None,
            vec![format!("A {mime} file can't be a {kind}.")],
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    if bytes.len() > max_size(&kind) {
        return editor_render(
            &s,
            &op,
            None,
            vec![format!(
                "That's {}; a {kind} can be {} at most.",
                crate::console::bytes(bytes.len() as i64),
                crate::console::bytes(max_size(&kind) as i64)
            )],
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    let id = hex::encode(rand::random::<[u8; 12]>());
    let key = format!("branding/{id}");
    // Bytes first, then the row.
    let stream = futures::stream::iter([Ok::<_, std::io::Error>(bytes.clone())]);
    if let Err(e) = s.blobs.write_stream(&key, stream, bytes.len() as u64).await {
        return editor_render(
            &s,
            &op,
            None,
            vec![format!("Storing it failed: {e}")],
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .await;
    }
    let row = AssetRow {
        id: id.clone(),
        kind: kind.clone(),
        name,
        mime: mime.into(),
        size: bytes.len() as i64,
        storage_key: key,
        created_at: 0,
        unused_since: None,
    };
    if let Err(e) = s.store.put_branding_asset(&row).await {
        return editor_render(
            &s,
            &op,
            None,
            vec![e.to_string()],
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .await;
    }
    let _ = s
        .store
        .audit(
            &op.actor(),
            "branding.upload",
            Some(&id),
            Some(&json!({"kind": kind, "mime": mime, "size": row.size})),
        )
        .await;
    web::redirect("/admin/branding?done=uploaded")
}

#[derive(Deserialize, Default)]
pub struct PreviewQuery {
    #[serde(default)]
    mode: String,
    #[serde(default)]
    kind: String,
}

fn mode_of(m: &str) -> Mode {
    match m {
        "dark" => Mode::Dark,
        "light" => Mode::Light,
        _ => Mode::Auto,
    }
}

/// `GET /admin/branding/draft.css?kind=vars|full&mode=`: the draft's
/// stylesheet, for previews (never cached, never shown to anyone else).
pub async fn draft_css(
    State(s): St,
    headers: HeaderMap,
    Query(q): Query<PreviewQuery>,
) -> Response {
    if require_op(&s, &headers).await.is_err() {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }
    let assets = assets_list(&s).await;
    let t = cleaned(&working_theme(&s).await, &assets);
    let mode = mode_of(&q.mode);
    let css = if q.kind == "full" {
        brand::full_css(&t, mode)
    } else {
        brand::vars_css(&t, mode)
    };
    css_response(css, "no-store")
}

/// `GET /admin/branding/preview/{page}?mode=`: a page with sample data in
/// the draft theme, framed by the editor.
pub async fn preview(
    State(s): St,
    headers: HeaderMap,
    Path(page): Path<String>,
    Query(q): Query<PreviewQuery>,
) -> Response {
    if require_op(&s, &headers).await.is_err() {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }
    let assets = assets_list(&s).await;
    let t = cleaned(&working_theme(&s).await, &assets);
    let mode = if q.mode == "dark" { "dark" } else { "light" };
    let css = |full: bool| {
        format!(
            "/admin/branding/draft.css?kind={}&mode={mode}",
            if full { "full" } else { "vars" }
        )
    };
    let name = if t.service_name.is_empty() {
        crate::console::brand(&s)
    } else {
        t.service_name.clone()
    };
    let html = match page.as_str() {
        "sign-in" | "consent" => crate::oauth_ui::preview_page(
            &s,
            page == "consent",
            &name,
            OauthBrand {
                css: css(false),
                logo: if t.logo.is_empty() {
                    String::new()
                } else {
                    brand::asset_path(&t.logo)
                },
                links: links(&t),
            },
        ),
        "sign-up" | "portal" => crate::portal::preview_page(
            &s,
            page == "sign-up",
            &name,
            dress_for(
                &t,
                css(true),
                if page == "sign-up" {
                    Blurb::SignUp
                } else {
                    Blurb::Portal
                },
            ),
            mode == "dark",
        ),
        _ => return (StatusCode::NOT_FOUND, "Not found").into_response(),
    };
    web::page_csp(
        html,
        StatusCode::OK,
        &locus_web::assets::CSP.replace("frame-ancestors 'none'", "frame-ancestors 'self'"),
    )
}

/// Collects uploads no version has used for a week.
pub async fn collect(s: &AppState) -> locus_core::CoreResult<u64> {
    s.store.collect_branding_assets(ASSET_GRACE_MS).await
}

/// The cached theme, without re-reading it (for code that can't wait;
/// callers refresh it with [`published`] earlier in the request).
#[must_use]
pub fn cached(s: &AppState) -> Option<Arc<Published>> {
    s.branding
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .and_then(|(_, p)| p.clone())
}

/// [`oauth_brand`] from the cached theme.
#[must_use]
pub fn oauth_brand_cached(s: &AppState, full: bool) -> OauthBrand {
    match cached(s) {
        Some(p) => OauthBrand {
            css: theme_href(p.version, full),
            logo: if p.theme.logo.is_empty() {
                String::new()
            } else {
                brand::asset_path(&p.theme.logo)
            },
            links: links(&p.theme),
        },
        None => OauthBrand::default(),
    }
}

/// [`dress`] from the cached theme.
#[must_use]
pub fn dress_cached(s: &AppState, blurb: Blurb) -> Option<Dress> {
    cached(s).map(|p| dress_for(&p.theme, theme_href(p.version, true), blurb))
}

/// The theme's sign-in heading for the portal, if it sets one.
#[must_use]
pub fn sign_in_heading(s: &AppState) -> String {
    cached(s)
        .map(|p| p.theme.text.sign_in_heading.clone())
        .unwrap_or_default()
}

/// [`service_name`] from the cached theme.
#[must_use]
pub fn service_name_cached(s: &AppState) -> String {
    match cached(s) {
        Some(p) if !p.theme.service_name.is_empty() => p.theme.service_name.clone(),
        _ => crate::console::brand(s),
    }
}
