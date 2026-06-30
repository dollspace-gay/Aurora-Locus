//! atproto-OAuth AS-login endpoint (Arc 2 Phase β.2, login-α; chainlink #420 /
//! LOCKED design §3.4).
//!
//! login-α authenticates a did:web holder to the browser-session substrate by
//! a DPoP-style challenge-response: the server issues a single-use nonce, and
//! the holder returns a secp256k1 (ES256K) signature over `SHA-256(nonce)`,
//! produced by the same `#atproto` key the substrate publishes for them. The
//! substrate verifies the signature against the stored
//! `did_web_account.identity_public_key` — it never holds the private key
//! (pre-decision 1). On success a browser session is minted (see
//! [`super::browser_session`]).
//!
//! The signature shape matches commit-signing (`RepoSigner::sign`): a 64-byte
//! compact `R‖S` ES256K signature over the SHA-256 prehash of the message, so
//! a holder client reuses its existing signing primitive. The signature and
//! challenge travel base64url in JSON.
//!
//! did:web only: a did:plc holder uses the existing JWT-session / app-password
//! paths, not this endpoint.

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use k256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

use super::browser_session;
use crate::context::AppContext;
use crate::error::{PdsError, PdsResult};
use crate::identity::did_method::is_web;

#[derive(Debug, Deserialize)]
pub struct ChallengeQuery {
    /// The did:web account requesting a login challenge.
    pub did: String,
}

#[derive(Debug, Serialize)]
pub struct ChallengeResponse {
    /// Single-use challenge nonce; the holder signs `SHA-256(challenge)`.
    pub challenge: String,
}

#[derive(Debug, Deserialize)]
pub struct VerifyRequest {
    /// The did:web account.
    pub did: String,
    /// The challenge nonce issued by `GET /oauth/atproto/login`.
    pub challenge: String,
    /// base64url(64-byte compact R‖S ES256K signature over SHA-256(challenge)).
    pub signature: String,
}

#[derive(Debug, Serialize)]
pub struct LoginSuccess {
    pub did: String,
    /// The session's anti-CSRF token, for the consent POSTs (β.3).
    pub csrf_token: String,
}

#[derive(Debug, Serialize)]
pub struct WhoamiResponse {
    pub did: String,
    pub csrf_token: String,
}

/// `GET /oauth/atproto/login?did=…` — issue a single-use login challenge.
///
/// Only the did:web *syntax* is checked here; account existence is **not**,
/// so this endpoint is not a DID-existence oracle. Existence + signature are
/// checked at [`verify`], which returns a uniform 401 on any failure.
pub async fn challenge(
    State(ctx): State<AppContext>,
    Query(q): Query<ChallengeQuery>,
) -> PdsResult<Json<ChallengeResponse>> {
    if !is_web(&q.did) {
        return Err(PdsError::Validation(
            "AS-login is available for did:web accounts only".to_string(),
        ));
    }
    let challenge = ctx.browser_login_nonces.generate_nonce().await;
    Ok(Json(ChallengeResponse { challenge }))
}

/// `POST /oauth/atproto/login` — verify a challenge response and mint a
/// browser session.
///
/// All failure modes collapse to a uniform 401 (`login failed`) to avoid
/// leaking whether the DID exists versus whether the signature was wrong.
/// The single exception is the did:web-only guard, which names the method
/// constraint (not an existence signal).
pub async fn verify(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(body): Json<VerifyRequest>,
) -> PdsResult<Response> {
    if !is_web(&body.did) {
        return Err(PdsError::Authentication(
            "AS-login is available for did:web accounts only".to_string(),
        ));
    }

    // Single-use challenge: consume it first so a replay cannot even reach the
    // (more expensive) signature check.
    if !ctx
        .browser_login_nonces
        .check_and_consume_nonce(&body.challenge)
        .await?
    {
        return Err(login_failed());
    }

    // Resolve the holder's published #atproto key. Unknown account → uniform
    // 401 (no existence oracle).
    let account = ctx
        .account_manager
        .get_did_web_account_by_did(&body.did)
        .await?
        .ok_or_else(login_failed)?;

    // Verify the holder signature over SHA-256(challenge) against the stored
    // public key. The substrate never holds the private key (pre-decision 1).
    verify_login_signature(&account.identity_public_key, &body.challenge, &body.signature)
        .map_err(|_| login_failed())?;

    // Session fixation defense: discard any pre-existing session before
    // minting a fresh one (the new cookie id is unrelated to the old).
    if let Some(old) = browser_session::read_session_cookie(&headers) {
        let _ = browser_session::delete_session(&ctx.account_db, &old).await;
    }

    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let session =
        browser_session::create_session(&ctx.account_db, &body.did, user_agent, None).await?;
    let cookie = browser_session::set_session_cookie(&session.id);

    Ok((
        [(header::SET_COOKIE, cookie)],
        Json(LoginSuccess {
            did: session.did,
            csrf_token: session.csrf_token,
        }),
    )
        .into_response())
}

/// `POST /oauth/atproto/logout` — delete the current session and clear the
/// cookie. Idempotent: succeeds whether or not a session was present.
pub async fn logout(State(ctx): State<AppContext>, headers: HeaderMap) -> Response {
    if let Some(id) = browser_session::read_session_cookie(&headers) {
        let _ = browser_session::delete_session(&ctx.account_db, &id).await;
    }
    (
        [(header::SET_COOKIE, browser_session::clear_session_cookie())],
        StatusCode::NO_CONTENT,
    )
        .into_response()
}

/// `GET /oauth/atproto/session` — return the authenticated holder for the
/// current browser session (the consumer that exercises the validation
/// extractor). 401 if there is no valid session.
pub async fn whoami(ctx: browser_session::BrowserSessionContext) -> Json<WhoamiResponse> {
    Json(WhoamiResponse {
        did: ctx.session.did,
        csrf_token: ctx.session.csrf_token,
    })
}

fn login_failed() -> PdsError {
    PdsError::Authentication("login failed".to_string())
}

/// Verify a login-α challenge signature: a 64-byte compact ES256K signature
/// over `SHA-256(challenge)`, against a multibase secp256k1 public key.
fn verify_login_signature(
    identity_public_key: &str,
    challenge: &str,
    signature_b64: &str,
) -> Result<(), ()> {
    let verifying_key = verifying_key_from_multibase(identity_public_key)?;
    let sig_bytes = URL_SAFE_NO_PAD.decode(signature_b64).map_err(|_| ())?;
    let signature = Signature::from_slice(&sig_bytes).map_err(|_| ())?;
    // `Verifier::verify` SHA-256-hashes the message internally, matching a
    // holder that signed SHA-256(challenge) (the commit-signing prehash shape).
    verifying_key
        .verify(challenge.as_bytes(), &signature)
        .map_err(|_| ())
}

/// Decode a multibase (did:key, base58btc, secp256k1 multicodec `0xe7 0x01`)
/// public key into a k256 `VerifyingKey`. `identity_public_key` is stored in
/// exactly this form (`crypto/secp256k1.rs::public_key_multibase`). Mirrors
/// the decode in `identity/resolver.rs` but yields a verify key directly.
fn verifying_key_from_multibase(multibase: &str) -> Result<VerifyingKey, ()> {
    let encoded = multibase.strip_prefix('z').ok_or(())?;
    let decoded = bs58::decode(encoded).into_vec().map_err(|_| ())?;
    // Strip the secp256k1-pub multicodec varint (`0xe7 0x01`) if present — the
    // W3C `publicKeyMultibase` form Aurora stores carries it. A compressed
    // SEC1 point begins with `0x02`/`0x03`, never `0xe7`, so this is
    // unambiguous and also accepts a bare-SEC1 multibase.
    let key_bytes = decoded
        .strip_prefix(&[0xe7u8, 0x01u8])
        .unwrap_or(decoded.as_slice());
    VerifyingKey::from_sec1_bytes(key_bytes).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::secp256k1::Secp256k1KeyPair;
    use axum::extract::{Query, State};
    use axum::http::{HeaderMap, StatusCode};
    use k256::ecdsa::{signature::Signer, Signature};

    /// publicKeyMultibase (W3C/did:key form, multicodec-prefixed) for a key.
    fn multibase_pubkey(kp: &Secp256k1KeyPair) -> String {
        kp.did().strip_prefix("did:key:").unwrap().to_string()
    }

    /// Sign `msg` the way a holder client would: ES256K over SHA-256(msg),
    /// 64-byte compact R‖S, base64url.
    fn sign_challenge(kp: &Secp256k1KeyPair, msg: &str) -> String {
        let sig: Signature = kp.signing_key().sign(msg.as_bytes());
        URL_SAFE_NO_PAD.encode(sig.to_bytes())
    }

    // ---------- crypto unit tests (no DB) ----------

    #[test]
    fn login_signature_roundtrips_multicodec_multibase() {
        let kp = Secp256k1KeyPair::generate();
        let challenge = "test-challenge-nonce-abc123";
        let sig = sign_challenge(&kp, challenge);
        assert!(verify_login_signature(&multibase_pubkey(&kp), challenge, &sig).is_ok());
    }

    #[test]
    fn login_signature_roundtrips_bare_sec1_multibase() {
        // The robust decoder also accepts the bare-SEC1 multibase form.
        let kp = Secp256k1KeyPair::generate();
        let challenge = "another-nonce";
        let sig = sign_challenge(&kp, challenge);
        assert!(verify_login_signature(&kp.public_key_multibase(), challenge, &sig).is_ok());
    }

    #[test]
    fn login_signature_rejects_wrong_message() {
        let kp = Secp256k1KeyPair::generate();
        let sig = sign_challenge(&kp, "the-real-challenge");
        // Signature is over a different message than the one verified.
        assert!(verify_login_signature(&multibase_pubkey(&kp), "a-different-challenge", &sig)
            .is_err());
    }

    #[test]
    fn login_signature_rejects_wrong_key() {
        let signer = Secp256k1KeyPair::generate();
        let other = Secp256k1KeyPair::generate();
        let challenge = "nonce";
        let sig = sign_challenge(&signer, challenge);
        // Verify against a different holder's published key.
        assert!(verify_login_signature(&multibase_pubkey(&other), challenge, &sig).is_err());
    }

    #[test]
    fn login_signature_rejects_garbage_inputs() {
        let kp = Secp256k1KeyPair::generate();
        assert!(verify_login_signature("not-multibase", "n", "AAAA").is_err());
        assert!(verify_login_signature(&multibase_pubkey(&kp), "n", "!!!notb64").is_err());
        assert!(verify_login_signature(&multibase_pubkey(&kp), "n", "AAAA").is_err());
    }

    // ---------- handler integration tests ----------

    async fn ctx() -> AppContext {
        crate::api::federation_peers::test_support::create_test_context_with(|_| {}).await
    }

    async fn seed_did_web(ctx: &AppContext, did: &str, slug: &str, pubkey_multibase: &str) {
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind(did)
            .bind(format!("{slug}.example.com"))
            .bind("2026-01-01T00:00:00Z")
            .execute(&ctx.account_db)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO did_web_account (did, domain, slug, identity_public_key, created_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(did)
        .bind("example.com")
        .bind(slug)
        .bind(pubkey_multibase)
        .bind("2026-01-01T00:00:00Z")
        .execute(&ctx.account_db)
        .await
        .unwrap();
    }

    async fn issue_challenge(ctx: &AppContext, did: &str) -> String {
        challenge(
            State(ctx.clone()),
            Query(ChallengeQuery {
                did: did.to_string(),
            }),
        )
        .await
        .expect("challenge issued")
        .0
        .challenge
    }

    #[tokio::test]
    async fn login_alpha_full_flow_mints_session() {
        let ctx = ctx().await;
        let kp = Secp256k1KeyPair::generate();
        let did = "did:web:alice.example.com";
        seed_did_web(&ctx, did, "alice", &multibase_pubkey(&kp)).await;

        let nonce = issue_challenge(&ctx, did).await;
        let sig = sign_challenge(&kp, &nonce);

        let resp = verify(
            State(ctx.clone()),
            HeaderMap::new(),
            Json(VerifyRequest {
                did: did.to_string(),
                challenge: nonce,
                signature: sig,
            }),
        )
        .await
        .expect("verify succeeds")
        .into_response();

        assert_eq!(resp.status(), StatusCode::OK);
        let set_cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(set_cookie.contains(browser_session::SESSION_COOKIE));
        assert!(set_cookie.contains("HttpOnly"));
        assert!(set_cookie.contains("SameSite=Lax"));
    }

    #[tokio::test]
    async fn login_alpha_replayed_nonce_is_rejected() {
        let ctx = ctx().await;
        let kp = Secp256k1KeyPair::generate();
        let did = "did:web:bob.example.com";
        seed_did_web(&ctx, did, "bob", &multibase_pubkey(&kp)).await;

        let nonce = issue_challenge(&ctx, did).await;
        let sig = sign_challenge(&kp, &nonce);
        let req = || VerifyRequest {
            did: did.to_string(),
            challenge: nonce.clone(),
            signature: sig.clone(),
        };

        assert!(verify(State(ctx.clone()), HeaderMap::new(), Json(req()))
            .await
            .is_ok());
        // Same nonce again — consumed, so a uniform 401.
        let err = verify(State(ctx.clone()), HeaderMap::new(), Json(req()))
            .await
            .unwrap_err();
        assert!(matches!(err, PdsError::Authentication(_)));
    }

    #[tokio::test]
    async fn login_alpha_bad_signature_is_rejected() {
        let ctx = ctx().await;
        let kp = Secp256k1KeyPair::generate();
        let did = "did:web:carol.example.com";
        seed_did_web(&ctx, did, "carol", &multibase_pubkey(&kp)).await;

        let nonce = issue_challenge(&ctx, did).await;
        // Sign a different message than the issued nonce.
        let wrong_sig = sign_challenge(&kp, "not-the-nonce");
        let err = verify(
            State(ctx.clone()),
            HeaderMap::new(),
            Json(VerifyRequest {
                did: did.to_string(),
                challenge: nonce,
                signature: wrong_sig,
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, PdsError::Authentication(_)));
    }

    #[tokio::test]
    async fn login_alpha_rejects_did_plc() {
        let ctx = ctx().await;
        let err = verify(
            State(ctx.clone()),
            HeaderMap::new(),
            Json(VerifyRequest {
                did: "did:plc:abc123".to_string(),
                challenge: "x".to_string(),
                signature: "AAAA".to_string(),
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, PdsError::Authentication(_)));
        // The challenge endpoint also refuses did:plc (400).
        let cerr = challenge(
            State(ctx.clone()),
            Query(ChallengeQuery {
                did: "did:plc:abc123".to_string(),
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(cerr, PdsError::Validation(_)));
    }

    #[tokio::test]
    async fn login_alpha_unknown_account_is_uniform_401() {
        let ctx = ctx().await;
        // Valid did:web syntax, but no account row, and a syntactically-fine
        // (but meaningless) signature. Must be the same 401 as a bad sig — no
        // existence oracle.
        let nonce = issue_challenge(&ctx, "did:web:ghost.example.com").await;
        let err = verify(
            State(ctx.clone()),
            HeaderMap::new(),
            Json(VerifyRequest {
                did: "did:web:ghost.example.com".to_string(),
                challenge: nonce,
                signature: URL_SAFE_NO_PAD.encode([0u8; 64]),
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, PdsError::Authentication(_)));
    }

    #[tokio::test]
    async fn whoami_and_logout_round_trip() {
        let ctx = ctx().await;
        let kp = Secp256k1KeyPair::generate();
        let did = "did:web:dave.example.com";
        seed_did_web(&ctx, did, "dave", &multibase_pubkey(&kp)).await;
        let nonce = issue_challenge(&ctx, did).await;
        let sig = sign_challenge(&kp, &nonce);
        let resp = verify(
            State(ctx.clone()),
            HeaderMap::new(),
            Json(VerifyRequest {
                did: did.to_string(),
                challenge: nonce,
                signature: sig,
            }),
        )
        .await
        .unwrap()
        .into_response();
        let cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let session_id = cookie
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches(&format!("{}=", browser_session::SESSION_COOKIE))
            .to_string();

        // whoami via the validation extractor resolves the holder.
        let session = browser_session::get_valid_session(&ctx.account_db, &session_id)
            .await
            .unwrap()
            .expect("session present");
        let who = whoami(browser_session::BrowserSessionContext { session }).await;
        assert_eq!(who.0.did, did);

        // logout deletes the row + clears the cookie.
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("{}={}", browser_session::SESSION_COOKIE, session_id)
                .parse()
                .unwrap(),
        );
        let lresp = logout(State(ctx.clone()), headers).await.into_response();
        assert_eq!(lresp.status(), StatusCode::NO_CONTENT);
        assert!(browser_session::get_valid_session(&ctx.account_db, &session_id)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn login_alpha_rotates_session_on_fixation() {
        let ctx = ctx().await;
        let kp = Secp256k1KeyPair::generate();
        let did = "did:web:erin.example.com";
        seed_did_web(&ctx, did, "erin", &multibase_pubkey(&kp)).await;

        // First login → session A.
        let n1 = issue_challenge(&ctx, did).await;
        let s1 = sign_challenge(&kp, &n1);
        let r1 = verify(
            State(ctx.clone()),
            HeaderMap::new(),
            Json(VerifyRequest {
                did: did.to_string(),
                challenge: n1,
                signature: s1,
            }),
        )
        .await
        .unwrap()
        .into_response();
        let id1 = cookie_id(&r1);

        // Second login presenting session A's cookie → session B; A is gone.
        let n2 = issue_challenge(&ctx, did).await;
        let s2 = sign_challenge(&kp, &n2);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("{}={}", browser_session::SESSION_COOKIE, id1)
                .parse()
                .unwrap(),
        );
        let r2 = verify(
            State(ctx.clone()),
            headers,
            Json(VerifyRequest {
                did: did.to_string(),
                challenge: n2,
                signature: s2,
            }),
        )
        .await
        .unwrap()
        .into_response();
        let id2 = cookie_id(&r2);

        assert_ne!(id1, id2, "session id must rotate");
        assert!(
            browser_session::get_valid_session(&ctx.account_db, &id1)
                .await
                .unwrap()
                .is_none(),
            "the old session must be discarded (fixation defense)"
        );
        assert!(browser_session::get_valid_session(&ctx.account_db, &id2)
            .await
            .unwrap()
            .is_some());
    }

    fn cookie_id(resp: &Response) -> String {
        let cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        cookie
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches(&format!("{}=", browser_session::SESSION_COOKIE))
            .to_string()
    }
}
