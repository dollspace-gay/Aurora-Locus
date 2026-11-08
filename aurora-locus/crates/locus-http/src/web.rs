//! Serving the web UI: its assets, and every page's response headers.

use axum::extract::Path;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use locus_web::assets;

/// `GET /static/{path}`: the stylesheet (by its hash) and the fonts.
pub async fn asset(Path(path): Path<String>) -> Response {
    let Some((kind, bytes)) = assets::asset(&path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut res = bytes.into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(assets::IMMUTABLE),
    );
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    res
}

/// `GET /`: the reference's plain banner, or (when turned on) the landing
/// page in the published theme.
pub async fn root(
    axum::extract::State(s): axum::extract::State<std::sync::Arc<crate::state::AppState>>,
) -> Response {
    let snap = s.snapshot();
    if !snap.bool("server.landing_page") {
        let mut res = BANNER.into_response();
        res.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );
        return res;
    }
    let portal = snap.bool("portal.enabled");
    drop(snap);
    let page = landing(
        &crate::branding::service_name(&s).await,
        crate::branding::dress(&s, crate::branding::Blurb::None).await,
        portal,
    );
    render(&page, StatusCode::OK)
}

/// The landing page, dressed in `dress` (the published theme, or a draft
/// for the branding editor's preview).
pub fn landing(
    name: &str,
    dress: Option<locus_web::view::Dress>,
    portal: bool,
) -> locus_web::view::LandingPage {
    locus_web::view::LandingPage {
        shell: locus_web::view::Shell {
            dress,
            ..locus_web::view::Shell::signed_out("Welcome", name)
        },
        portal,
        sign_up: portal,
    }
}

/// `GET /robots.txt`, as the reference.
pub async fn robots() -> Response {
    let mut res =
        "# Hello!\n\n# Crawling the public API is allowed\nUser-agent: *\nAllow: /".into_response();
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    res
}

/// The reference's banner at `/`.
const BANNER: &str = r"
         __                         __
        /\ \__                     /\ \__
    __  \ \ ,_\  _____   _ __   ___\ \ ,_\   ___
  /'__'\ \ \ \/ /\ '__'\/\''__\/ __'\ \ \/  / __'\
 /\ \L\.\_\ \ \_\ \ \L\ \ \ \//\ \L\ \ \ \_/\ \L\ \
 \ \__/.\_\\ \__\\ \ ,__/\ \_\\ \____/\ \__\ \____/
  \/__/\/_/ \/__/ \ \ \/  \/_/ \/___/  \/__/\/___/
                   \ \_\
                    \/_/


This is an AT Protocol Personal Data Server (aka, an atproto PDS)

Most API routes are under /xrpc/

      Code: https://github.com/bluesky-social/atproto
 Self-Host: https://github.com/bluesky-social/pds
  Protocol: https://atproto.com
";

/// A rendered page with the UI's headers: the strict CSP, no framing, no
/// caching, no referrer beyond this site.
pub fn page(html: String, status: StatusCode) -> Response {
    let mut res = (status, html).into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(assets::CSP),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(assets::NO_STORE),
    );
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    res
}

/// Renders a template into a page; a template that fails to render is a
/// plain 500 rather than a half page.
pub fn render(t: &impl askama::Template, status: StatusCode) -> Response {
    match t.render() {
        Ok(html) => page(html, status),
        Err(e) => {
            tracing::error!(error = %e, "a page failed to render");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// [`render`] for a page whose shell may be live: a live one gets the CSP
/// that lets its one script run and listen.
pub fn render_shell(
    t: &impl askama::Template,
    shell: &locus_web::view::Shell,
    status: StatusCode,
) -> Response {
    if shell.live.is_empty() {
        return render(t, status);
    }
    match t.render() {
        Ok(html) => page_csp(html, status, assets::CSP_LIVE),
        Err(e) => {
            tracing::error!(error = %e, "a page failed to render");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// A `303 See Other` to `to`, uncached.
pub fn redirect(to: &str) -> Response {
    let mut res = StatusCode::SEE_OTHER.into_response();
    if let Ok(v) = HeaderValue::from_str(to) {
        res.headers_mut().insert(header::LOCATION, v);
    }
    res.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(assets::NO_STORE),
    );
    res
}

/// Adds a `Set-Cookie`.
pub fn with_cookie(mut res: Response, cookie: &str) -> Response {
    if let Ok(v) = HeaderValue::from_str(cookie) {
        res.headers_mut().append(header::SET_COOKIE, v);
    }
    res
}

/// The request's cookies.
pub fn cookies(headers: &axum::http::HeaderMap) -> Vec<(String, String)> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|p| {
            p.trim()
                .split_once('=')
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
        })
        .collect()
}

pub fn cookie<'a>(all: &'a [(String, String)], name: &str) -> Option<&'a str> {
    all.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// A form body's fields, in order.
pub fn parse_form(body: &[u8]) -> Vec<(String, String)> {
    url::form_urlencoded::parse(body)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

/// The first value of a field, trimmed; empty when absent.
pub fn field<'a>(form: &'a [(String, String)], name: &str) -> &'a str {
    form.iter()
        .find(|(k, _)| k == name)
        .map_or("", |(_, v)| v.trim())
}

/// Every value of a field (checkbox groups).
pub fn fields<'a>(form: &'a [(String, String)], name: &str) -> Vec<&'a str> {
    form.iter()
        .filter(|(k, _)| k == name)
        .map(|(_, v)| v.trim())
        .collect()
}

/// Lines (or commas) of a text area, trimmed, empty ones dropped.
pub fn lines(s: &str) -> Vec<String> {
    s.split(['\n', ','])
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Whether a form post came from this site (browsers send
/// `Sec-Fetch-Site`; one that says otherwise is refused).
pub fn same_site(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .is_none_or(|v| v == "same-origin")
}

/// Constant-time string equality.
pub fn ct_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    !a.is_empty() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

/// A page with its own `Content-Security-Policy` (the branding editor
/// frames its previews; the previews may be framed by it).
pub fn page_csp(html: String, status: StatusCode, csp: &str) -> Response {
    let mut res = page(html, status);
    if let Ok(v) = HeaderValue::from_str(csp) {
        res.headers_mut().insert(header::CONTENT_SECURITY_POLICY, v);
    }
    if csp.contains("frame-ancestors 'self'") {
        res.headers_mut().insert(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("SAMEORIGIN"),
        );
    }
    res
}
