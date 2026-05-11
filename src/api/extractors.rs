//! Custom axum extractors for Aurora-Locus's API surface.
//!
//! Currently this module hosts `AuroraJson<T>`: a drop-in
//! replacement for `axum::Json<T>` on admin-tier handlers that
//! emits a structured JSON rejection envelope (rather than
//! axum's default `text/plain` body) when the request body
//! fails to deserialize.
//!
//! ## Why
//!
//! Arc 6 Step 1 shipped a translation layer at
//! `static/admin/scripts/api/error-translations.js` that maps
//! server `{error, message}` JSON envelopes to operator-
//! friendly prose. The translation layer is consulted by
//! `static/admin/scripts/api/client.js`'s 4xx handling path.
//!
//! axum's default `Json<T>` extractor rejects malformed bodies
//! with a `(StatusCode, String)` response (Content-Type:
//! text/plain). The plain-text rejection bypasses the JSON
//! envelope path entirely — operators sending malformed input
//! see the raw axum diagnostic text instead of translated prose.
//!
//! `AuroraJson<T>` wraps `axum::Json<T>::from_request` and
//! converts its rejection into a JSON envelope shaped like
//! every other 4xx response Aurora-Locus emits:
//!
//! ```json
//! { "error": "InvalidRequestBody", "message": "<axum's original diagnostic>" }
//! ```
//!
//! The `InvalidRequestBody` code is seeded in
//! `error-translations.js` (Arc 6 Phase B fix-up) so operators
//! see "The request body has invalid structure..." rather than
//! the raw "Failed to deserialize the JSON body…" diagnostic.
//!
//! ## Migration scope (Arc 6 Phase B fix-up)
//!
//! The fix-up migrates the two dual-shape endpoints' handlers
//! (`emit_event`, `update_subject_status`) per the kickoff's
//! "ship the dual-shape sites only; remainder becomes a v0.6
//! candidate" guidance. The pattern is mechanical (`Json(input):
//! Json<T>` → `AuroraJson(input): AuroraJson<T>`); the broader
//! sweep across the remaining ~36 admin handler sites is a v0.6
//! candidate, documented in `docs/v06-candidates.md`.

use axum::{
    async_trait,
    extract::{FromRequest, Request},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::de::DeserializeOwned;

/// Wrapper around `axum::Json<T>` that converts the default
/// `text/plain` deserialization-rejection response into a JSON
/// envelope (`{"error": "InvalidRequestBody", "message": ...}`).
///
/// Destructure the same way you would `axum::Json`:
///
/// ```ignore
/// pub async fn handler(
///     AuroraJson(input): AuroraJson<MyInput>,
/// ) -> Result<Json<MyOutput>, ...> { ... }
/// ```
///
/// Successful deserialization is a transparent passthrough —
/// the inner type is identical to what `axum::Json<T>` would
/// produce. Only the rejection path differs.
#[derive(Debug)]
pub struct AuroraJson<T>(pub T);

#[async_trait]
impl<S, T> FromRequest<S> for AuroraJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = AuroraJsonRejection;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(AuroraJson(value)),
            Err(rejection) => Err(AuroraJsonRejection {
                status: rejection.status(),
                message: rejection.body_text(),
            }),
        }
    }
}

/// Rejection type for `AuroraJson<T>`. Implements `IntoResponse`
/// to emit the canonical `{"error", "message"}` envelope.
pub struct AuroraJsonRejection {
    status: StatusCode,
    message: String,
}

impl IntoResponse for AuroraJsonRejection {
    fn into_response(self) -> Response {
        let body = serde_json::json!({
            "error": "InvalidRequestBody",
            "message": self.message,
        });
        (self.status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request as HttpRequest, StatusCode};
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct TestInput {
        #[allow(dead_code)]
        name: String,
        #[allow(dead_code)]
        count: i64,
    }

    async fn extract_body(resp: Response) -> (StatusCode, String) {
        let status = resp.status();
        let body_bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body_str = String::from_utf8_lossy(&body_bytes).to_string();
        (status, body_str)
    }

    fn json_request(body: &str) -> HttpRequest<Body> {
        HttpRequest::builder()
            .method("POST")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn aurora_json_passes_through_well_formed_body() {
        let req = json_request(r#"{"name": "test", "count": 7}"#);
        let result = AuroraJson::<TestInput>::from_request(req, &()).await;
        assert!(result.is_ok(), "well-formed body should parse cleanly");
    }

    #[tokio::test]
    async fn aurora_json_rejection_emits_json_envelope_on_missing_field() {
        // Missing required field `count` — axum's Json<T> would
        // reject with text/plain "Failed to deserialize…".
        let req = json_request(r#"{"name": "test"}"#);
        let rejection = AuroraJson::<TestInput>::from_request(req, &())
            .await
            .expect_err("missing field must reject");
        let resp = rejection.into_response();
        let (status, body) = extract_body(resp).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "missing-field rejections are 422 per axum convention"
        );
        let parsed: serde_json::Value = serde_json::from_str(&body)
            .expect("rejection body must be valid JSON, not text/plain");
        assert_eq!(parsed["error"], "InvalidRequestBody");
        assert!(
            parsed["message"].as_str().unwrap().contains("count"),
            "axum's original diagnostic should be preserved in the message; got: {}",
            parsed["message"]
        );
    }

    #[tokio::test]
    async fn aurora_json_rejection_emits_json_envelope_on_syntax_error() {
        // Syntactically invalid JSON — axum's Json<T> would
        // reject with a syntax-error variant of JsonRejection.
        let req = json_request(r#"{"name": "test", count}"#);
        let rejection = AuroraJson::<TestInput>::from_request(req, &())
            .await
            .expect_err("invalid syntax must reject");
        let resp = rejection.into_response();
        let (status, body) = extract_body(resp).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "syntax-error rejections are 400 per axum convention"
        );
        let parsed: serde_json::Value = serde_json::from_str(&body)
            .expect("rejection body must be valid JSON");
        assert_eq!(parsed["error"], "InvalidRequestBody");
    }

    #[tokio::test]
    async fn aurora_json_rejection_emits_json_envelope_on_missing_content_type() {
        // No Content-Type header — axum's Json<T> rejects with
        // MissingJsonContentType (415 Unsupported Media Type).
        let req = HttpRequest::builder()
            .method("POST")
            .body(Body::from(r#"{"name": "test", "count": 7}"#.to_string()))
            .unwrap();
        let rejection = AuroraJson::<TestInput>::from_request(req, &())
            .await
            .expect_err("missing content-type must reject");
        let resp = rejection.into_response();
        let (status, body) = extract_body(resp).await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let parsed: serde_json::Value = serde_json::from_str(&body)
            .expect("rejection body must be valid JSON");
        assert_eq!(parsed["error"], "InvalidRequestBody");
    }
}
