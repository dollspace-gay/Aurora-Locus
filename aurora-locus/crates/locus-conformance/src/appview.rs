//! A mock AppView both servers proxy to. Every XRPC call is echoed back:
//! method, NSID, query, the service-auth claims (`iss`, `aud`, `lxm`), the
//! forwarded headers and a JSON body, so what each server sent can be
//! compared. `?fail=500` / `?fail=400` answer with that error;
//! `resolveHandle` and `repo.getRecord` answer as an AppView that doesn't
//! know the subject.

use std::collections::BTreeMap;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, Query};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use base64::Engine;
use serde_json::{Value, json};

const FORWARDED: &[&str] = &[
    "accept-language",
    "atproto-accept-labelers",
    "x-bsky-topics",
    "atproto-proxy",
    "content-type",
];

fn claims(headers: &HeaderMap) -> Value {
    let Some(token) = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return Value::Null;
    };
    let payload = token.split('.').nth(1).unwrap_or_default();
    let c: Value = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    json!({"iss": c["iss"], "aud": c["aud"], "lxm": c["lxm"], "exp": c["exp"].is_number()})
}

async fn xrpc(
    method: Method,
    Path(nsid): Path<String>,
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match query.get("fail").map(String::as_str) {
        Some("500") => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({"error": "InternalServerError", "message": "mock failure"})),
            )
                .into_response();
        }
        Some("400") => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({"error": "InvalidRequest", "message": "mock refusal"})),
            )
                .into_response();
        }
        _ => {}
    }
    match nsid.as_str() {
        "com.atproto.identity.resolveHandle" => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(
                    json!({"error": "InvalidRequest", "message": "Unable to resolve handle"}),
                ),
            )
                .into_response();
        }
        "com.atproto.repo.getRecord" => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(
                    json!({"error": "RecordNotFound", "message": "Could not locate record"}),
                ),
            )
                .into_response();
        }
        _ => {}
    }
    let forwarded: BTreeMap<&str, &str> = FORWARDED
        .iter()
        .filter_map(|h| Some((*h, headers.get(*h)?.to_str().ok()?)))
        .collect();
    let json_body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (
        StatusCode::OK,
        [("content-language", "en")],
        axum::Json(json!({
            "echo": {
                "method": method.as_str(),
                "nsid": nsid,
                "query": query,
                "auth": claims(&headers),
                "headers": forwarded,
                "body": json_body,
            },
            "feed": [],
        })),
    )
        .into_response()
}

/// Serves until the process ends.
pub async fn serve(port: u16) -> std::io::Result<()> {
    let app = Router::new().route("/xrpc/{nsid}", any(xrpc));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    axum::serve(listener, app).await
}
