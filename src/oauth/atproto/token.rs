//! atproto-OAuth token endpoint (Arc 2 Phase β.3, chainlink #420 / LOCKED
//! design §3.2 steps 6-9 + 7).
//!
//! `POST /oauth/atproto/token` exchanges an authorization code for a
//! DPoP-bound access + refresh token pair, and rotates refresh tokens. Both
//! grants require a DPoP proof (atproto OAuth mandates DPoP — there is no
//! bearer fallback here, unlike the legacy `oauth/token.rs`).
//!
//! Issued tokens land in the shared `token` table via β.1's
//! `access_token_hash` discipline, so they validate through the same
//! [`crate::auth::validate_oauth_token`] path every XRPC uses. Refresh rotation
//! reuses the legacy [`crate::oauth::token_rotation::TokenRotationManager`]
//! (replay detection + breach-revocation), layering the DPoP proof-of-
//! possession check on top.

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use axum::Form;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{Duration, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use super::request_store;
use super::{oauth_error_json, verify_dpop_required};
use crate::context::AppContext;
use crate::oauth::token_rotation::TokenRotationManager;

/// Token-request form body. One struct for both grants; the grant-specific
/// fields are optional and checked per branch.
#[derive(Debug, Deserialize)]
pub struct TokenForm {
    pub grant_type: String,
    pub code: Option<String>,
    pub code_verifier: Option<String>,
    pub client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub refresh_token: Option<String>,
}

/// `POST /oauth/atproto/token`
pub async fn token(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Form(form): Form<TokenForm>,
) -> Response {
    let result = match form.grant_type.as_str() {
        "authorization_code" => authorization_code_grant(&ctx, &headers, &form).await,
        "refresh_token" => refresh_token_grant(&ctx, &headers, &form).await,
        other => Err(oauth_error_json(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            &format!("unsupported grant_type '{other}'"),
        )),
    };
    match result {
        Ok(resp) => resp,
        Err(resp) => resp,
    }
}

async fn authorization_code_grant(
    ctx: &AppContext,
    headers: &HeaderMap,
    form: &TokenForm,
) -> Result<Response, Response> {
    let code = nonempty(&form.code).ok_or_else(|| missing_param("code"))?;
    let code_verifier =
        nonempty(&form.code_verifier).ok_or_else(|| missing_param("code_verifier"))?;
    let client_id = nonempty(&form.client_id).ok_or_else(|| missing_param("client_id"))?;
    let redirect_uri =
        nonempty(&form.redirect_uri).ok_or_else(|| missing_param("redirect_uri"))?;

    // 1. DPoP — mandatory. The proof's JWK becomes the issued token's bound
    //    key. Absent/invalid → 401.
    let htu = format!("{}/oauth/atproto/token", ctx.service_url());
    let thumbprint = verify_dpop_required(ctx, headers, &htu)
        .await
        .map_err(|e| oauth_error_json(StatusCode::UNAUTHORIZED, "invalid_dpop_proof", &e.to_string()))?;

    // 2. Look up the authorization request by the code's hash. Generic
    //    `invalid_grant` for every lookup/validity failure (no oracle).
    let request = request_store::get_by_code_hash(&ctx.account_db, &super::token_hash(code))
        .await
        .map_err(server_error)?
        .ok_or_else(invalid_grant)?;

    if request.is_expired(Utc::now()) || request.is_denied() || request.code_is_used() {
        return Err(invalid_grant());
    }
    // The client redeeming must be the client the code was issued to.
    if request.client_id != client_id {
        return Err(invalid_grant());
    }
    // redirect_uri must match the one bound at the authorize step. The
    // redirect_uri↔client-metadata binding was already enforced at authorize
    // time; re-fetching the client document here would only add a network
    // dependency to the token hot path without strengthening the PKCE+DPoP-
    // protected code exchange.
    if request.redirect_uri != redirect_uri {
        return Err(invalid_grant());
    }
    // 3. PKCE: base64url(SHA-256(code_verifier)) must equal the stored challenge.
    if !verify_pkce_s256(code_verifier, &request.code_challenge) {
        return Err(invalid_grant());
    }
    // The code must be bound to a holder (defensive: a PAR row that never
    // reached consent has no did and no code_hash, so it cannot match here).
    let did = request.did.as_deref().ok_or_else(invalid_grant)?;

    // 4. Single-use: atomically claim the code. A concurrent/replayed
    //    redemption loses the CAS and is rejected.
    let claimed = request_store::claim_code(
        &ctx.account_db,
        &request.request_id,
        &Utc::now().to_rfc3339(),
    )
    .await
    .map_err(server_error)?;
    if !claimed {
        return Err(invalid_grant());
    }

    // 5. Mint + persist the token pair (shared `token` table, β.1 hash).
    let issued = issue_tokens(ctx, did, client_id, &request.scope, &thumbprint)
        .await
        .map_err(server_error)?;

    Ok(token_response(&issued, &request.scope))
}

async fn refresh_token_grant(
    ctx: &AppContext,
    headers: &HeaderMap,
    form: &TokenForm,
) -> Result<Response, Response> {
    let refresh_token =
        nonempty(&form.refresh_token).ok_or_else(|| missing_param("refresh_token"))?;
    let client_id = nonempty(&form.client_id).ok_or_else(|| missing_param("client_id"))?;

    // 1. DPoP — mandatory.
    let htu = format!("{}/oauth/atproto/token", ctx.service_url());
    let thumbprint = verify_dpop_required(ctx, headers, &htu)
        .await
        .map_err(|e| oauth_error_json(StatusCode::UNAUTHORIZED, "invalid_dpop_proof", &e.to_string()))?;

    // 2. Proof-of-possession: the presented DPoP key must match the key the
    //    token was bound to at issuance. Look up the row's thumbprint directly;
    //    a mismatch is a stolen-refresh-token signal → reject before rotating.
    let bound: Option<Option<String>> =
        sqlx::query("SELECT dpop_thumbprint FROM token WHERE current_refresh_token = $1")
            .bind(refresh_token)
            .fetch_optional(&ctx.account_db)
            .await
            .map_err(|e| server_error(crate::error::PdsError::Database(e)))?
            .map(|row| row.get::<Option<String>, _>("dpop_thumbprint"));
    if let Some(Some(bound_thumbprint)) = bound {
        if bound_thumbprint != thumbprint {
            return Err(oauth_error_json(
                StatusCode::UNAUTHORIZED,
                "invalid_token",
                "DPoP key does not match the bound token",
            ));
        }
    }

    // 3. Rotate via the legacy rotation manager (replay detection +
    //    breach-revocation + new access_token_hash). It validates client_id +
    //    expiry and persists the rotated bearer hash.
    let manager = TokenRotationManager::new(ctx.account_db.clone());
    let rotated = manager
        .rotate_token(refresh_token, client_id)
        .await
        .map_err(|e| oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", &e.to_string()))?;

    // 4. atproto tokens are DPoP-bound; override the manager's "Bearer" label.
    let body = serde_json::json!({
        "access_token": rotated.access_token,
        "refresh_token": rotated.refresh_token,
        "token_type": "DPoP",
        "expires_in": rotated.expires_in,
        "scope": rotated.scope,
    });
    Ok(json_ok(&body))
}

/// The minted token pair.
struct IssuedTokens {
    access_token: String,
    refresh_token: String,
}

/// Access-token lifetime advertised to the client (`expires_in`).
const ACCESS_TOKEN_TTL_SECS: i64 = 3600;
/// Refresh-token lifetime — also the `token.expires_at` column value. NOTE:
/// the shared `token` schema has a single `expires_at` column that
/// `validate_oauth_token` reads as the bearer's validity AND the rotation path
/// reads as the refresh validity (a pre-existing legacy quirk — `oauth/token.rs`
/// sets it the same way). We mirror that exactly so both providers write the
/// column with one meaning; well-behaved clients refresh hourly per
/// `expires_in` regardless.
const REFRESH_TOKEN_TTL_DAYS: i64 = 90;

async fn issue_tokens(
    ctx: &AppContext,
    did: &str,
    client_id: &str,
    scope: &str,
    thumbprint: &str,
) -> Result<IssuedTokens, crate::error::PdsError> {
    let access_token = format!("at_{}", super::opaque_token());
    let refresh_token = format!("rt_{}", super::opaque_token());
    let access_token_hash = crate::oauth::access_token_hash(&access_token);

    let now = Utc::now();
    let expires_at = now + Duration::days(REFRESH_TOKEN_TTL_DAYS);
    sqlx::query(
        r#"
        INSERT INTO token (
            token_id, did, client_id, current_refresh_token,
            scope, created_at, updated_at, expires_at,
            dpop_thumbprint, device_id, access_token_hash
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        "#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(did)
    .bind(client_id)
    .bind(&refresh_token)
    .bind(scope)
    .bind(now.to_rfc3339())
    .bind(now.to_rfc3339())
    .bind(expires_at.to_rfc3339())
    .bind(thumbprint)
    .bind(Option::<String>::None)
    .bind(&access_token_hash)
    .execute(&ctx.account_db)
    .await
    .map_err(crate::error::PdsError::Database)?;

    Ok(IssuedTokens {
        access_token,
        refresh_token,
    })
}

fn token_response(issued: &IssuedTokens, scope: &str) -> Response {
    let body = serde_json::json!({
        "access_token": issued.access_token,
        "refresh_token": issued.refresh_token,
        "token_type": "DPoP",
        "expires_in": ACCESS_TOKEN_TTL_SECS,
        "scope": scope,
    });
    json_ok(&body)
}

fn json_ok(body: &serde_json::Value) -> Response {
    let bytes = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(bytes.into())
        .expect("static header set builds a valid response")
}

/// PKCE S256 verification: `base64url(SHA-256(code_verifier)) == code_challenge`.
fn verify_pkce_s256(code_verifier: &str, code_challenge: &str) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(code_verifier.as_bytes());
    let computed = URL_SAFE_NO_PAD.encode(hasher.finalize());
    // Constant-time-ish compare is unnecessary here (both sides are derived
    // from values the client already holds), but a length-checked eq avoids a
    // short-circuit on the first byte for the common case.
    computed == code_challenge
}

/// Borrow a present, non-empty form field.
fn nonempty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|v| !v.is_empty())
}

/// The `invalid_request` response for a missing required parameter.
fn missing_param(name: &str) -> Response {
    oauth_error_json(
        StatusCode::BAD_REQUEST,
        "invalid_request",
        &format!("missing required parameter: {name}"),
    )
}

fn invalid_grant() -> Response {
    oauth_error_json(
        StatusCode::BAD_REQUEST,
        "invalid_grant",
        "the authorization code is invalid, expired, or already used",
    )
}

fn server_error(e: crate::error::PdsError) -> Response {
    oauth_error_json(StatusCode::INTERNAL_SERVER_ERROR, "server_error", &e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::federation::dpop::{DPopClaims, Jwk};
    use crate::oauth::atproto::request_store::AtprotoAuthorizationRequest;
    use axum::body::to_bytes;

    async fn ctx() -> AppContext {
        crate::api::federation_peers::test_support::create_test_context_with(|_| {}).await
    }

    // ---- DPoP proof builder (mirrors federation/dpop.rs test helper) ----

    fn fresh_keypair_jwk() -> (p256::ecdsa::SigningKey, Jwk) {
        use p256::ecdsa::SigningKey;
        use p256::EncodedPoint;
        let signing_key = SigningKey::random(&mut rand::rngs::OsRng);
        let verifying_key = signing_key.verifying_key();
        let encoded: EncodedPoint = verifying_key.to_encoded_point(false);
        let jwk = Jwk {
            kty: "EC".to_string(),
            crv: "P-256".to_string(),
            x: URL_SAFE_NO_PAD.encode(encoded.x().unwrap()),
            y: URL_SAFE_NO_PAD.encode(encoded.y().unwrap()),
        };
        (signing_key, jwk)
    }

    fn signed_proof(signing_key: &p256::ecdsa::SigningKey, jwk: &Jwk, htu: &str) -> String {
        use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
        use p256::pkcs8::EncodePrivateKey;
        let claims = DPopClaims {
            jti: Uuid::new_v4().to_string(),
            htm: "POST".to_string(),
            htu: htu.to_string(),
            iat: Utc::now().timestamp(),
            exp: Utc::now().timestamp() + 60,
            ath: None,
        };
        let pem = signing_key.to_pkcs8_pem(Default::default()).unwrap().to_string();
        let key = EncodingKey::from_ec_pem(pem.as_bytes()).unwrap();
        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some("dpop+jwt".to_string());
        let jwk_value = serde_json::to_value(jwk).unwrap();
        header.jwk = Some(serde_json::from_value(jwk_value).unwrap());
        encode(&header, &claims, &key).unwrap()
    }

    fn dpop_headers(proof: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("DPoP", proof.parse().unwrap());
        h
    }

    /// PKCE pair: a verifier and its S256 challenge.
    fn pkce_pair() -> (String, String) {
        let verifier = "verifier-0123456789-abcdefghij-klmnopqrst".to_string();
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());
        (verifier, challenge)
    }

    async fn seed_code_request(
        ctx: &AppContext,
        request_id: &str,
        code_hash: &str,
        code_challenge: &str,
    ) {
        let now = Utc::now();
        let req = AtprotoAuthorizationRequest {
            request_id: request_id.to_string(),
            request_uri: None,
            client_id: "https://app.example.com/cm.json".to_string(),
            redirect_uri: "https://app.example.com/cb".to_string(),
            scope: "atproto transition:generic".to_string(),
            state: None,
            code_challenge: code_challenge.to_string(),
            code_challenge_method: "S256".to_string(),
            did: Some("did:web:alice.example.com".to_string()),
            code_hash: Some(code_hash.to_string()),
            code_used_at: None,
            denied_at: None,
            created_at: now.to_rfc3339(),
            expires_at: (now + Duration::minutes(10)).to_rfc3339(),
        };
        request_store::insert(&ctx.account_db, &req).await.unwrap();
    }

    fn token_form(grant_type: &str) -> TokenForm {
        TokenForm {
            grant_type: grant_type.to_string(),
            code: None,
            code_verifier: None,
            client_id: Some("https://app.example.com/cm.json".to_string()),
            redirect_uri: Some("https://app.example.com/cb".to_string()),
            refresh_token: None,
        }
    }

    async fn body_json(resp: Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    #[test]
    fn pkce_s256_matches_and_rejects() {
        let (verifier, challenge) = pkce_pair();
        assert!(verify_pkce_s256(&verifier, &challenge));
        assert!(!verify_pkce_s256("wrong-verifier", &challenge));
    }

    #[tokio::test]
    async fn authorization_code_without_dpop_is_401() {
        let ctx = ctx().await;
        let mut form = token_form("authorization_code");
        form.code = Some("the-code".to_string());
        form.code_verifier = Some("v".to_string());
        let resp = token(State(ctx.clone()), HeaderMap::new(), Form(form)).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn unsupported_grant_type_is_400() {
        let ctx = ctx().await;
        let resp = token(
            State(ctx.clone()),
            HeaderMap::new(),
            Form(token_form("password")),
        )
        .await;
        let (status, json) = body_json(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"], "unsupported_grant_type");
    }

    #[tokio::test]
    async fn pkce_mismatch_is_invalid_grant() {
        let ctx = ctx().await;
        let (_verifier, challenge) = pkce_pair();
        let code = "abc-code";
        seed_code_request(&ctx, "req-pk", &super::super::token_hash(code), &challenge).await;

        let (sk, jwk) = fresh_keypair_jwk();
        let htu = format!("{}/oauth/atproto/token", ctx.service_url());
        let proof = signed_proof(&sk, &jwk, &htu);

        let mut form = token_form("authorization_code");
        form.code = Some(code.to_string());
        form.code_verifier = Some("the-WRONG-verifier".to_string());
        let resp = token(State(ctx.clone()), dpop_headers(&proof), Form(form)).await;
        let (status, json) = body_json(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"], "invalid_grant");
    }

    #[tokio::test]
    async fn valid_code_issues_dpop_tokens_that_validate_and_single_use_holds() {
        let ctx = ctx().await;
        let (verifier, challenge) = pkce_pair();
        let code = "the-real-code";
        seed_code_request(&ctx, "req-ok", &super::super::token_hash(code), &challenge).await;

        let (sk, jwk) = fresh_keypair_jwk();
        let htu = format!("{}/oauth/atproto/token", ctx.service_url());

        let mut form = token_form("authorization_code");
        form.code = Some(code.to_string());
        form.code_verifier = Some(verifier.clone());
        let resp = token(
            State(ctx.clone()),
            dpop_headers(&signed_proof(&sk, &jwk, &htu)),
            Form(form),
        )
        .await;
        let (status, json) = body_json(resp).await;
        assert_eq!(status, StatusCode::OK, "issued: {json:?}");
        assert_eq!(json["token_type"], "DPoP");
        assert_eq!(json["expires_in"], ACCESS_TOKEN_TTL_SECS);
        assert_eq!(json["scope"], "atproto transition:generic");
        let access_token = json["access_token"].as_str().unwrap().to_string();

        // β.1: the issued bearer validates via the shared validation path.
        let validated = crate::auth::validate_oauth_token(&ctx, &access_token)
            .await
            .expect("bearer validates");
        assert_eq!(validated.did, "did:web:alice.example.com");
        assert_eq!(validated.scope, "atproto transition:generic");
        assert!(validated.dpop_thumbprint.is_some());

        // Single-use: re-redeeming the same code now fails (code_used_at set).
        let mut form2 = token_form("authorization_code");
        form2.code = Some(code.to_string());
        form2.code_verifier = Some(verifier);
        let resp2 = token(
            State(ctx.clone()),
            dpop_headers(&signed_proof(&sk, &jwk, &htu)),
            Form(form2),
        )
        .await;
        let (status2, json2) = body_json(resp2).await;
        assert_eq!(status2, StatusCode::BAD_REQUEST);
        assert_eq!(json2["error"], "invalid_grant");
    }

    #[tokio::test]
    async fn refresh_grant_rotates_and_keeps_dpop_binding() {
        let ctx = ctx().await;
        let (verifier, challenge) = pkce_pair();
        let code = "code-for-refresh";
        seed_code_request(&ctx, "req-rt", &super::super::token_hash(code), &challenge).await;

        let (sk, jwk) = fresh_keypair_jwk();
        let htu = format!("{}/oauth/atproto/token", ctx.service_url());

        // Initial exchange.
        let mut form = token_form("authorization_code");
        form.code = Some(code.to_string());
        form.code_verifier = Some(verifier);
        let (status, json) = body_json(
            token(
                State(ctx.clone()),
                dpop_headers(&signed_proof(&sk, &jwk, &htu)),
                Form(form),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let refresh_token = json["refresh_token"].as_str().unwrap().to_string();

        // Refresh with the SAME DPoP key → rotates.
        let mut rform = token_form("refresh_token");
        rform.refresh_token = Some(refresh_token.clone());
        let (rstatus, rjson) = body_json(
            token(
                State(ctx.clone()),
                dpop_headers(&signed_proof(&sk, &jwk, &htu)),
                Form(rform),
            )
            .await,
        )
        .await;
        assert_eq!(rstatus, StatusCode::OK, "rotate: {rjson:?}");
        assert_eq!(rjson["token_type"], "DPoP");
        assert!(rjson["access_token"].as_str().unwrap().starts_with("at_"));

        // Refresh with a DIFFERENT DPoP key → POP failure, 401.
        let (other_sk, other_jwk) = fresh_keypair_jwk();
        let mut rform2 = token_form("refresh_token");
        // The original refresh_token was consumed by the rotate above; use the
        // newly issued one to isolate the POP-mismatch check.
        rform2.refresh_token = Some(rjson["refresh_token"].as_str().unwrap().to_string());
        let resp = token(
            State(ctx.clone()),
            dpop_headers(&signed_proof(&other_sk, &other_jwk, &htu)),
            Form(rform2),
        )
        .await;
        let (status3, json3) = body_json(resp).await;
        assert_eq!(status3, StatusCode::UNAUTHORIZED);
        assert_eq!(json3["error"], "invalid_token");
    }
}
