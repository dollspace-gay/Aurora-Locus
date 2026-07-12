//! atproto-OAuth consent endpoints (Arc 2 Phase β.3, chainlink #420 / LOCKED
//! design §3.2 / R1 F-3.2).
//!
//! `POST /oauth/atproto/consent/approve` and `.../deny` are the resource
//! owner's decision on the consent screen rendered by [`super::authorize`].
//! Both are gated by a valid browser session (login-α) AND enforce the
//! resource-owner authorization invariant (F-3.2): the session's CSRF token
//! must match, and the session DID must equal the DID bound on the
//! authorization request. The authorization `request_id` is a correlation key,
//! never a trust token — possession of it grants nothing without the matching
//! session.
//!
//! Approve mints a single-use authorization code (its hash stored; the raw
//! code travels only in the redirect) and 302s to the client's redirect URI.
//! Deny tombstones the request and 302s with `error=access_denied`.

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Form;
use chrono::Utc;
use serde::Deserialize;

use super::browser_session::BrowserSessionContext;
use super::request_store::{self, AtprotoAuthorizationRequest};
use crate::context::AppContext;

/// The consent form body: the correlation key + the session CSRF token.
#[derive(Debug, Deserialize)]
pub struct ConsentForm {
    pub request_id: String,
    pub csrf_token: String,
}

/// `POST /oauth/atproto/consent/approve`
pub async fn approve(
    session: BrowserSessionContext,
    State(ctx): State<AppContext>,
    Form(form): Form<ConsentForm>,
) -> Response {
    let request = match precheck(&ctx, &session, &form).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    // Mint the authorization code; persist only its hash (the raw code travels
    // solely in the redirect). code_used_at stays NULL until token redemption.
    let code = super::opaque_token();
    let code_hash = super::token_hash(&code);
    if let Err(e) = request_store::set_code_hash(&ctx.account_db, &request.request_id, &code_hash)
        .await
    {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }

    // 302 to the client with the code (+ echoed state).
    let mut pairs: Vec<(&str, &str)> = vec![("code", &code)];
    if let Some(state) = request.state.as_deref() {
        pairs.push(("state", state));
    }
    redirect_to_client(&request.redirect_uri, &pairs)
}

/// `POST /oauth/atproto/consent/deny`
pub async fn deny(
    session: BrowserSessionContext,
    State(ctx): State<AppContext>,
    Form(form): Form<ConsentForm>,
) -> Response {
    let request = match precheck(&ctx, &session, &form).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    if let Err(e) =
        request_store::mark_denied(&ctx.account_db, &request.request_id, &Utc::now().to_rfc3339())
            .await
    {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }

    // 302 to the client with the OAuth access_denied error (+ echoed state).
    let mut pairs: Vec<(&str, &str)> = vec![("error", "access_denied")];
    if let Some(state) = request.state.as_deref() {
        pairs.push(("state", state));
    }
    redirect_to_client(&request.redirect_uri, &pairs)
}

/// The shared gate for both consent decisions: CSRF + request lookup +
/// liveness + the session-DID == request-DID invariant (F-3.2).
async fn precheck(
    ctx: &AppContext,
    session: &BrowserSessionContext,
    form: &ConsentForm,
) -> Result<AtprotoAuthorizationRequest, Response> {
    // CSRF: the form token must match the session's per-session token. No hint
    // about whether the session or the token was at fault.
    if form.csrf_token != session.session.csrf_token {
        return Err(fail(StatusCode::FORBIDDEN, "CSRF token mismatch"));
    }

    let request = request_store::get_by_request_id(&ctx.account_db, &form.request_id)
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "unknown authorization request"))?;

    if request.is_expired(Utc::now()) {
        return Err(fail(StatusCode::NOT_FOUND, "authorization request expired"));
    }
    if request.code_is_used() {
        return Err(fail(StatusCode::BAD_REQUEST, "authorization already completed"));
    }
    if request.is_denied() {
        return Err(fail(StatusCode::BAD_REQUEST, "authorization already denied"));
    }

    // F-3.2: the resource owner deciding consent must be the same holder the
    // request was bound to at the authorize step.
    if request.did.as_deref() != Some(session.session.did.as_str()) {
        return Err(fail(
            StatusCode::FORBIDDEN,
            "session does not match this authorization request",
        ));
    }

    Ok(request)
}

/// 302 to the client's redirect URI with the given query pairs appended,
/// preserving any query already present on the registered redirect URI.
///
/// `pub(super)` so the authorize handler's first-party admin auto-approve
/// (chainlink #439) issues its code through the exact same redirect path.
pub(super) fn redirect_to_client(redirect_uri: &str, pairs: &[(&str, &str)]) -> Response {
    let location = match url::Url::parse(redirect_uri) {
        Ok(mut url) => {
            url.query_pairs_mut().extend_pairs(pairs.iter().copied());
            url.to_string()
        }
        // The redirect_uri was verified against client metadata upstream, so
        // this branch is defensive; fall back to manual composition.
        Err(_) => {
            let sep = if redirect_uri.contains('?') { '&' } else { '?' };
            let query: String = pairs
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{}={}",
                        k,
                        url::form_urlencoded::byte_serialize(v.as_bytes()).collect::<String>()
                    )
                })
                .collect::<Vec<_>>()
                .join("&");
            format!("{redirect_uri}{sep}{query}")
        }
    };
    (StatusCode::FOUND, [(header::LOCATION, location)]).into_response()
}

fn fail(status: StatusCode, msg: &str) -> Response {
    (status, msg.to_string()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::atproto::browser_session::{self, BrowserSession};
    use crate::oauth::atproto::request_store::AtprotoAuthorizationRequest;
    use chrono::Duration;

    async fn ctx() -> AppContext {
        crate::api::federation_peers::test_support::create_test_context_with(|_| {}).await
    }

    async fn seed_session(ctx: &AppContext, did: &str) -> BrowserSession {
        browser_session::create_session(&ctx.account_db, did, None, None)
            .await
            .unwrap()
    }

    async fn seed_request(ctx: &AppContext, request_id: &str, did: Option<&str>) {
        let now = Utc::now();
        let req = AtprotoAuthorizationRequest {
            request_id: request_id.to_string(),
            request_uri: None,
            client_id: "https://app.example.com/cm.json".to_string(),
            redirect_uri: "https://app.example.com/cb".to_string(),
            scope: "atproto".to_string(),
            state: Some("st-1".to_string()),
            code_challenge: "chal".to_string(),
            code_challenge_method: "S256".to_string(),
            did: did.map(|d| d.to_string()),
            code_hash: None,
            code_used_at: None,
            denied_at: None,
            created_at: now.to_rfc3339(),
            expires_at: (now + Duration::minutes(10)).to_rfc3339(),
        };
        request_store::insert(&ctx.account_db, &req).await.unwrap();
    }

    fn session_ctx(session: BrowserSession) -> BrowserSessionContext {
        BrowserSessionContext { session }
    }

    #[tokio::test]
    async fn approve_with_csrf_mismatch_is_403() {
        let ctx = ctx().await;
        let session = seed_session(&ctx, "did:web:alice.example.com").await;
        seed_request(&ctx, "req-a", Some("did:web:alice.example.com")).await;
        let resp = approve(
            session_ctx(session),
            State(ctx.clone()),
            Form(ConsentForm {
                request_id: "req-a".to_string(),
                csrf_token: "wrong".to_string(),
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn approve_with_did_mismatch_is_403() {
        let ctx = ctx().await;
        let session = seed_session(&ctx, "did:web:alice.example.com").await;
        // Request bound to a DIFFERENT holder.
        seed_request(&ctx, "req-b", Some("did:web:bob.example.com")).await;
        let csrf = session.csrf_token.clone();
        let resp = approve(
            session_ctx(session),
            State(ctx.clone()),
            Form(ConsentForm {
                request_id: "req-b".to_string(),
                csrf_token: csrf,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn approve_valid_redirects_with_code_and_state() {
        let ctx = ctx().await;
        let session = seed_session(&ctx, "did:web:alice.example.com").await;
        seed_request(&ctx, "req-c", Some("did:web:alice.example.com")).await;
        let csrf = session.csrf_token.clone();
        let resp = approve(
            session_ctx(session),
            State(ctx.clone()),
            Form(ConsentForm {
                request_id: "req-c".to_string(),
                csrf_token: csrf,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FOUND);
        let location = resp.headers().get(header::LOCATION).unwrap().to_str().unwrap();
        assert!(location.starts_with("https://app.example.com/cb?"));
        assert!(location.contains("code="));
        assert!(location.contains("state=st-1"));

        // The code's hash was persisted (single-use machinery armed).
        let row = request_store::get_by_request_id(&ctx.account_db, "req-c")
            .await
            .unwrap()
            .unwrap();
        assert!(row.code_hash.is_some());
        assert!(!row.code_is_used());
    }

    #[tokio::test]
    async fn second_approve_of_completed_request_is_400() {
        let ctx = ctx().await;
        let session = seed_session(&ctx, "did:web:alice.example.com").await;
        seed_request(&ctx, "req-d", Some("did:web:alice.example.com")).await;
        // Simulate the code already redeemed.
        request_store::set_code_hash(&ctx.account_db, "req-d", "h").await.unwrap();
        request_store::claim_code(&ctx.account_db, "req-d", &Utc::now().to_rfc3339())
            .await
            .unwrap();
        let csrf = session.csrf_token.clone();
        let resp = approve(
            session_ctx(session),
            State(ctx.clone()),
            Form(ConsentForm {
                request_id: "req-d".to_string(),
                csrf_token: csrf,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn deny_valid_redirects_with_access_denied() {
        let ctx = ctx().await;
        let session = seed_session(&ctx, "did:web:alice.example.com").await;
        seed_request(&ctx, "req-e", Some("did:web:alice.example.com")).await;
        let csrf = session.csrf_token.clone();
        let resp = deny(
            session_ctx(session),
            State(ctx.clone()),
            Form(ConsentForm {
                request_id: "req-e".to_string(),
                csrf_token: csrf,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FOUND);
        let location = resp.headers().get(header::LOCATION).unwrap().to_str().unwrap();
        assert!(location.contains("error=access_denied"));
        assert!(location.contains("state=st-1"));

        let row = request_store::get_by_request_id(&ctx.account_db, "req-e")
            .await
            .unwrap()
            .unwrap();
        assert!(row.is_denied());
    }
}
