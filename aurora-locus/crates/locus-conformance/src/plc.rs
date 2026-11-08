//! A mock PLC directory both servers resolve against: `POST /{did}`
//! stores an operation; `GET /{did}/log/last` and `/{did}/data` return it;
//! `GET /{did}` returns the DID document (404 once tombstoned).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::{Value, json};

type Ops = Arc<Mutex<HashMap<String, Value>>>;

fn document(did: &str, op: &Value) -> Value {
    let key = op["verificationMethods"]["atproto"]
        .as_str()
        .unwrap_or_default()
        .trim_start_matches("did:key:");
    let services: Vec<Value> = op["services"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| json!({"id": format!("#{k}"), "type": v["type"], "serviceEndpoint": v["endpoint"]}))
                .collect()
        })
        .unwrap_or_default();
    json!({
        "@context": [
            "https://www.w3.org/ns/did/v1",
            "https://w3id.org/security/multikey/v1",
            "https://w3id.org/security/suites/secp256k1-2019/v1",
        ],
        "id": did,
        "alsoKnownAs": op["alsoKnownAs"],
        "verificationMethod": [{
            "id": format!("{did}#atproto"),
            "type": "Multikey",
            "controller": did,
            "publicKeyMultibase": key,
        }],
        "service": services,
    })
}

async fn get_any(State(ops): State<Ops>, Path(path): Path<String>) -> Response {
    let (did, raw) = match path
        .strip_suffix("/log/last")
        .or_else(|| path.strip_suffix("/data"))
    {
        Some(d) => (d.to_owned(), true),
        None => (path.clone(), false),
    };
    let op = ops.lock().ok().and_then(|m| m.get(&did).cloned());
    match op {
        Some(op) if raw => axum::Json(op).into_response(),
        Some(op) if op["type"] != "plc_tombstone" => {
            axum::Json(document(&did, &op)).into_response()
        }
        _ => (StatusCode::NOT_FOUND, axum::Json(json!({}))).into_response(),
    }
}

async fn post_op(
    State(ops): State<Ops>,
    Path(did): Path<String>,
    axum::Json(op): axum::Json<Value>,
) -> StatusCode {
    if let Ok(mut m) = ops.lock() {
        m.insert(did, op);
    }
    StatusCode::OK
}

/// Serves until the process ends.
pub async fn serve(port: u16) -> std::io::Result<()> {
    let ops: Ops = Arc::default();
    let app = Router::new()
        .route("/{*path}", get(get_any).post(post_op))
        .with_state(ops);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    axum::serve(listener, app).await
}
