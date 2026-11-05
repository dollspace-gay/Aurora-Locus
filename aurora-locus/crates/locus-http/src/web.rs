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
