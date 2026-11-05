//! `app.bsky.*` methods locus answers itself: preferences, and (with an
//! AppView configured) read-after-write on the AppView's views.

use std::sync::Arc;

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use locus_auth::session::{SCOPE_ACCESS, SCOPE_TAKENDOWN};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth;
use crate::error::{XrpcError, XrpcResult};
use crate::pipethrough as pt;
use crate::state::AppState;

type St = State<Arc<AppState>>;

const NAMESPACE: &str = "app.bsky";
const PERSONAL_DETAILS: &str = "app.bsky.actor.defs#personalDetailsPref";
const DECLARED_AGE: &str = "app.bsky.actor.defs#declaredAgePref";

/// The `atproto-proxy` header, when it names another service than the
/// configured AppView (such requests are piped through, as the reference).
fn proxied_elsewhere(s: &AppState, headers: &HeaderMap) -> bool {
    let Some(v) = headers.get("atproto-proxy").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    pt::appview(s).is_none_or(|(_, did)| v != format!("{did}#bsky_appview"))
}

fn accept_encoding(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

/// Pipes a request through to the `atproto-proxy` service as `iss`.
async fn pipe(
    s: &AppState,
    lxm: &str,
    method: Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: Option<Body>,
    iss: &str,
) -> Response {
    let target = match pt::target(s, headers, lxm).await {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    let sent = pt::send(
        s,
        pt::Outgoing {
            method,
            lxm,
            path_and_query: uri.path_and_query().map_or(uri.path(), |q| q.as_str()),
            headers,
            body,
            iss: Some(iss),
            aud: None,
            target: &target,
            accept_encoding: accept_encoding(headers),
            strict: false,
        },
    )
    .await;
    match sent {
        Ok(up) => pt::stream_back(s, up),
        Err(r) => r,
    }
}

/// Age in whole years on today's UTC date, as the reference's
/// `getAgeFromDatestring`; None if the date doesn't parse.
fn age_from(birth: &str) -> Option<i64> {
    let ms = if birth.contains('T') {
        crate::account::parse_datetime_ms(birth)?
    } else {
        crate::account::parse_datetime_ms(&format!("{birth}T00:00:00Z"))?
    };
    let ymd = |ms: i64| -> Option<(i64, i64, i64)> {
        let s = locus_core::time::iso8601(ms);
        Some((
            s.get(0..4)?.parse().ok()?,
            s.get(5..7)?.parse().ok()?,
            s.get(8..10)?.parse().ok()?,
        ))
    };
    let (by, bm, bd) = ymd(ms)?;
    let (ty, tm, td) = ymd(locus_core::time::now_ms())?;
    let mut age = ty - by;
    if tm < bm || (tm == bm && td < bd) {
        age -= 1;
    }
    Some(age)
}

/// The account's `app.bsky` preferences as the reference returns them: a
/// synthetic `declaredAgePref` after a `personalDetailsPref` with a
/// birth date, and `personalDetailsPref` only with full access.
async fn read_preferences(s: &AppState, did: &str, full: bool) -> XrpcResult<Vec<Value>> {
    let mut prefs: Vec<Value> = s
        .store
        .preferences(did, NAMESPACE)
        .await?
        .into_iter()
        .filter_map(|(_, json)| serde_json::from_str(&json).ok())
        .collect();
    let birth = prefs
        .iter()
        .find(|p| p["$type"] == PERSONAL_DETAILS)
        .and_then(|p| p["birthDate"].as_str())
        .filter(|b| !b.is_empty())
        .map(str::to_owned);
    if let Some(b) = birth {
        // An unparseable date answers false everywhere, as NaN does there.
        let age = age_from(&b);
        let over = |n: i64| age.is_some_and(|a| a >= n);
        prefs.push(json!({
            "$type": DECLARED_AGE,
            "isOverAge13": over(13),
            "isOverAge16": over(16),
            "isOverAge18": over(18),
        }));
    }
    Ok(prefs
        .into_iter()
        .filter(|p| full || p["$type"] != PERSONAL_DETAILS)
        .collect())
}

#[derive(Deserialize)]
pub struct GetPreferencesParams {
    did: Option<String>,
}

/// `app.bsky.actor.getPreferences`. Served locally whether or not an
/// AppView is configured; piped through when `atproto-proxy` names another
/// service. The moderation service may read any account's (`did`).
pub async fn get_preferences(
    State(s): St,
    uri: Uri,
    headers: HeaderMap,
    Query(p): Query<GetPreferencesParams>,
) -> Response {
    get_preferences_inner(&s, &uri, &headers, p)
        .await
        .unwrap_or_else(IntoResponse::into_response)
}

async fn get_preferences_inner(
    s: &AppState,
    uri: &Uri,
    headers: &HeaderMap,
    p: GetPreferencesParams,
) -> XrpcResult<Response> {
    let lxm = "app.bsky.actor.getPreferences";
    let token = auth::bearer(headers)?;
    let (did, full) = if locus_auth::service_jwt::parse(token, None, 0).is_ok() {
        let iss = auth::user_service_auth(s, headers, lxm).await?;
        let moderation = s.snapshot().text("moderation.service_did").to_owned();
        if moderation.is_empty() || iss != moderation {
            return Err(XrpcError::auth_required("Untrusted issuer"));
        }
        if proxied_elsewhere(s, headers) {
            return Err(XrpcError::invalid_request(
                "Moderator requests cannot be proxied to other app views",
            ));
        }
        let did = p
            .did
            .filter(|d| proto_blue_syntax::Did::new(d).is_ok())
            .ok_or_else(|| XrpcError::invalid_request("Invalid or missing did parameter"))?;
        (did, true)
    } else {
        let authed = auth::access_with(s, headers, auth::STANDARD, &[SCOPE_TAKENDOWN]).await?;
        if proxied_elsewhere(s, headers) {
            return Ok(pipe(s, lxm, Method::GET, uri, headers, None, &authed.did).await);
        }
        let full = authed.scope == SCOPE_ACCESS;
        (authed.did, full)
    };
    let preferences = read_preferences(s, &did, full).await?;
    Ok(Json(json!({ "preferences": preferences })).into_response())
}

/// `app.bsky.actor.putPreferences`: replaces the account's `app.bsky`
/// preferences (those the credential may see), as the reference.
pub async fn put_preferences(State(s): St, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    put_preferences_inner(&s, &uri, &headers, body)
        .await
        .unwrap_or_else(IntoResponse::into_response)
}

async fn put_preferences_inner(
    s: &AppState,
    uri: &Uri,
    headers: &HeaderMap,
    body: Bytes,
) -> XrpcResult<Response> {
    let lxm = "app.bsky.actor.putPreferences";
    let authed = auth::access(s, headers).await?;
    auth::account_checked(s, &authed.did, false).await?;
    if proxied_elsewhere(s, headers) {
        return Ok(pipe(
            s,
            lxm,
            Method::POST,
            uri,
            headers,
            Some(Body::from(body)),
            &authed.did,
        )
        .await);
    }
    let input: Value = serde_json::from_slice(&body)
        .map_err(|_| XrpcError::invalid_request("Request body must be JSON"))?;
    let items = input
        .get("preferences")
        .ok_or_else(|| XrpcError::invalid_request("Input must have the property \"preferences\""))?
        .as_array()
        .ok_or_else(|| XrpcError::invalid_request("Input/preferences must be an array"))?;
    let mut prefs: Vec<(String, Value)> = Vec::with_capacity(items.len());
    for item in items {
        let Some(t) = item.get("$type").and_then(Value::as_str) else {
            return Err(XrpcError::invalid_request("Preference is missing a $type"));
        };
        prefs.push((t.to_owned(), item.clone()));
    }
    if !prefs
        .iter()
        .all(|(t, _)| locus_store::prefs::in_namespace(NAMESPACE, t))
    {
        return Err(XrpcError::invalid_request(format!(
            "Some preferences are not in the {NAMESPACE} namespace"
        )));
    }
    let full = authed.scope == SCOPE_ACCESS;
    let forbidden: Vec<&str> = prefs
        .iter()
        .filter(|(t, _)| !full && t == PERSONAL_DETAILS)
        .map(|(t, _)| t.as_str())
        .collect();
    if !forbidden.is_empty() {
        return Err(XrpcError::invalid_request(format!(
            "Do not have authorization to set preferences: {}",
            forbidden.join(", ")
        )));
    }
    let values: Vec<(String, String)> = prefs
        .into_iter()
        .filter(|(t, _)| t != DECLARED_AGE)
        .map(|(t, v)| (t, v.to_string()))
        .collect();
    let mut tx = s.store.begin().await?;
    tx.replace_preferences(
        &authed.did,
        NAMESPACE,
        |name| full || name != PERSONAL_DETAILS,
        &values,
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::OK.into_response())
}

#[cfg(test)]
mod tests {
    use super::age_from;

    #[test]
    fn ages() {
        let now = locus_core::time::iso8601(locus_core::time::now_ms());
        let year: i64 = now[..4].parse().unwrap();
        let today = &now[4..10];
        assert_eq!(
            age_from(&format!("{}{today}T00:00:00.000Z", year - 18)),
            Some(18)
        );
        assert_eq!(age_from(&format!("{}{today}", year - 13)), Some(13));
        assert_eq!(age_from("not a date"), None);
    }
}
