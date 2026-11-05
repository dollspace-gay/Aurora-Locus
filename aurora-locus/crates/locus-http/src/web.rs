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
