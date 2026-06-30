//! Entryway-mode header builders (Arc 12 §5.4 Step 2.1 + 2.2,
//! consumed by §5.3.8 forwarded handlers).
//!
//! Two builders:
//!
//! - [`entryway_auth_headers`] (mint pattern, §5.4 Step 2.1):
//!   used by `signPlcOperation`, `updateHandle`, and `getSession`
//!   when entryway mode is configured. Looks up the user's atproto
//!   signing key, mints a fresh ES256K service-auth JWT scoped to
//!   the specific lexicon method (`lxm`), and packs it into an
//!   `Authorization: Bearer …` header. Per §5.3.5: TTL ≤60s, never
//!   cached, re-minted per forward.
//!
//! - [`entryway_passthru_headers`] (Step 2.2): lands separately
//!   alongside the §5.3.6 enumerated header set.
//!
//! Failure modes for Step 2.1 follow the locked design's matrix:
//! `UnknownAccount` when no `plc_keys` row exists for `user_did`,
//! `KeyNotFound` when the row exists but `atproto_signing_key` is
//! the empty-string default left by legacy rows (pre-Step-1.5).

use crate::error::{PdsError, PdsResult};
use axum::http::{HeaderMap, HeaderValue};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use k256::ecdsa::{signature::Signer, Signature, SigningKey};
use serde_json::json;
use sqlx::AnyPool;
use std::net::SocketAddr;
use uuid::Uuid;

/// kid value Aurora-Locus stamps on every JWT it mints
/// (access tokens per Step 0.6.2; service-auth tokens here).
/// Routed by §5.3.3's tuple table.
pub const AURORA_LOCAL_KID: &str = "aurora-local-v1";

/// §5.4 Step 2.1 — mint a service-auth JWT for forwarding `lxm`
/// to the entryway, returned as a ready-to-attach `HeaderMap`.
///
/// Flow:
/// 1. `SELECT atproto_signing_key FROM plc_keys WHERE did = $user_did`.
///    Missing row → `UnknownAccount`; row-but-empty-column →
///    `KeyNotFound` (legacy pre-Step-1.5 rows surface here until
///    Arc 13 v4.1 §6.3.2 forward-populates them).
/// 2. Hex-decode → 32-byte k256 private key.
/// 3. Construct claims: `iss = user_did`, `aud = entryway_did`,
///    `lxm`, `iat = now`, `exp = now + 60`, `jti` (fresh UUIDv4
///    for entryway-side replay tracking).
/// 4. Header: `{"alg":"ES256K","typ":"JWT","kid":"aurora-local-v1"}`.
/// 5. Sign over `header_b64.payload_b64` with k256 ECDSA, DER-encode
///    signature.
/// 6. Pack into `Authorization: Bearer <jwt>` HeaderMap and return.
///
/// `entryway_did` is the locally-configured entryway DID
/// (`AppContext::entryway_did()`); caller supplies it to avoid
/// circular ctx deps.
///
/// Per §5.3.5: TTL is fixed at 60s, JWT is never cached, and the
/// caller re-mints per forward.
/// Read the per-account atproto signing key (hex) for an entryway-auth JWT,
/// method-aware (v0.10 Arc 1 Phase A — R1 D-1 inline-SQL migration / R2 F-3).
///
/// did:plc reads `plc_keys.atproto_signing_key` (the historical inline path,
/// lifted into this accessor). did:web is **rejected**: a public-key-only
/// did:web account has no substrate-held signing key, and signing an
/// entryway-auth JWT as the holder would be a sovereignty break (LOCKED Arc 1
/// §3 / §10 — same single-key reasoning as commit signing). Holder-mediation is
/// Arc 2 territory. A malformed DID is rejected rather than read as an empty key.
async fn entryway_signing_key_hex(db: &AnyPool, user_did: &str) -> PdsResult<String> {
    use crate::identity::did_method::{parse_did, DidMethod};
    match parse_did(user_did).map(|p| p.method()) {
        Ok(DidMethod::Plc) => {
            let row: Option<(String,)> =
                sqlx::query_as("SELECT atproto_signing_key FROM plc_keys WHERE did = $1")
                    .bind(user_did)
                    .fetch_optional(db)
                    .await
                    .map_err(PdsError::Database)?;
            match row {
                Some((key,)) if !key.is_empty() => Ok(key),
                Some(_) => Err(PdsError::Authentication(format!(
                    "KeyNotFound: plc_keys.atproto_signing_key is empty for {} \
                     (legacy row pending Arc 13 v4.1 §6.3.2 forward-population)",
                    user_did
                ))),
                None => Err(PdsError::NotFound(format!(
                    "UnknownAccount: no plc_keys row for {}",
                    user_did
                ))),
            }
        }
        Ok(DidMethod::Web) => Err(PdsError::Validation(format!(
            "entryway-auth JWT is not available for did:web account {}: the substrate \
             holds no signing key (holder-mediated signing is forthcoming in Arc 2)",
            user_did
        ))),
        Err(e) => Err(PdsError::Validation(format!(
            "unparseable DID {}: {}",
            user_did, e
        ))),
    }
}

pub async fn entryway_auth_headers(
    db: &AnyPool,
    user_did: &str,
    entryway_did: &str,
    lxm: &str,
) -> PdsResult<HeaderMap> {
    let signing_key_hex = entryway_signing_key_hex(db, user_did).await?;

    let key_bytes = hex::decode(&signing_key_hex).map_err(|e| {
        PdsError::Internal(format!(
            "plc_keys.atproto_signing_key for {} is not valid hex: {}",
            user_did, e
        ))
    })?;
    let signing_key = SigningKey::from_slice(&key_bytes).map_err(|e| {
        PdsError::Internal(format!(
            "plc_keys.atproto_signing_key for {} is not a valid k256 private key: {}",
            user_did, e
        ))
    })?;

    let now = chrono::Utc::now().timestamp();
    let header = json!({
        "alg": "ES256K",
        "typ": "JWT",
        "kid": AURORA_LOCAL_KID,
    });
    let claims = json!({
        "iss": user_did,
        "aud": entryway_did,
        "lxm": lxm,
        "iat": now,
        "exp": now + 60,
        "jti": Uuid::new_v4().to_string(),
    });

    let header_json = serde_json::to_string(&header)
        .map_err(|e| PdsError::Jwt(format!("serialize entryway-auth header: {}", e)))?;
    let claims_json = serde_json::to_string(&claims)
        .map_err(|e| PdsError::Jwt(format!("serialize entryway-auth claims: {}", e)))?;
    let header_b64 = URL_SAFE_NO_PAD.encode(header_json.as_bytes());
    let claims_b64 = URL_SAFE_NO_PAD.encode(claims_json.as_bytes());
    let signing_input = format!("{}.{}", header_b64, claims_b64);
    let signature: Signature = signing_key.sign(signing_input.as_bytes());
    let sig_b64 = URL_SAFE_NO_PAD.encode(signature.to_der().as_bytes());
    let token = format!("{}.{}", signing_input, sig_b64);

    let mut h = HeaderMap::new();
    let value = HeaderValue::from_str(&format!("Bearer {}", token)).map_err(|e| {
        PdsError::Internal(format!("invalid Authorization header value: {}", e))
    })?;
    h.insert(axum::http::header::AUTHORIZATION, value);
    Ok(h)
}

/// §5.4 Step 2.2 — passthru-pattern header filter for entryway
/// forwarding (e.g., `requestPasswordReset`).
///
/// **Source-of-truth pinning (§5.3.6 staleness gate).** Read against
/// the bsky-PDS source on 2026-05-18, current `main`:
/// - `packages/pds/src/context.ts::entrywayPassthruHeaders(req)`
///   → `forwardedFor(req, authPassthru(req))`.
/// - `packages/pds/src/api/proxy.ts::authPassthru(req)`: copies
///   incoming `authorization` (rejecting DPoP-typed auth or
///   requests bearing a `dpop` header with `InvalidRequest`).
/// - `packages/pds/src/api/proxy.ts::forwardedFor(req, params)`:
///   sets `x-forwarded-for` from `req.ip`.
///
/// **Delta from V05_DESIGN.md §5.3.6's enumerated set.** v4.1's list
/// (Authorization, Content-Type, Accept-Language, User-Agent,
/// Atproto-Proxy, DPoP, Idempotency-Key, X-Forwarded-For,
/// X-Forwarded-Host) is broader than what current bsky-PDS actually
/// forwards. Per §5.3.6's verification gate clause 4 ("default is to
/// follow current bsky-PDS pattern"), this implementation tracks
/// bsky-PDS: `authorization` + `x-forwarded-for` only, with DPoP
/// rejection.
///
/// Routes whose semantics need the broader v4.1 set will surface
/// the gap in Phase B integration testing; §5.3.6 can be revised at
/// that point with concrete drivers rather than speculative coverage.
///
/// **Failure mode (`InvalidRequest`).** When the incoming request
/// has `authorization: DPoP …` or has a `dpop` header alongside any
/// authorization, the bsky-PDS source rejects with
/// `InvalidRequestError('DPoP requests cannot be proxied')` —
/// matched here by `PdsError::Validation(...)`.
///
/// `remote_addr` is the client connection address (axum
/// `ConnectInfo<SocketAddr>` at the handler boundary). When the
/// incoming request already carries `x-forwarded-for` set by a
/// reverse proxy, that value is preserved verbatim; otherwise the
/// connection address is written.
pub fn entryway_passthru_headers(
    incoming: &HeaderMap,
    remote_addr: Option<SocketAddr>,
) -> PdsResult<HeaderMap> {
    let mut out = HeaderMap::new();

    if let Some(auth) = incoming.get(axum::http::header::AUTHORIZATION) {
        let auth_str = auth.to_str().map_err(|_| {
            PdsError::Validation("Authorization header is not valid ASCII".to_string())
        })?;
        let scheme_is_dpop = auth_str
            .split_whitespace()
            .next()
            .map(|s| s.eq_ignore_ascii_case("DPoP"))
            .unwrap_or(false);
        if scheme_is_dpop || incoming.contains_key("dpop") {
            return Err(PdsError::Validation(
                "DPoP requests cannot be proxied".to_string(),
            ));
        }
        out.insert(axum::http::header::AUTHORIZATION, auth.clone());
    }

    if let Some(existing_xff) = incoming.get("x-forwarded-for") {
        out.insert("x-forwarded-for", existing_xff.clone());
    } else if let Some(addr) = remote_addr {
        let xff_val = HeaderValue::from_str(&addr.ip().to_string()).map_err(|e| {
            PdsError::Internal(format!("invalid X-Forwarded-For value: {}", e))
        })?;
        out.insert("x-forwarded-for", xff_val);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    // ---------- entryway_auth_headers tests (§5.4 Step 2.1) ----------

    async fn seed_plc_keys(db: &AnyPool, did: &str, _rotation: &str, atproto: &str) {
        // Arc 13 Step 0.7.1 dropped the rotation_key column; the
        // `_rotation` param is retained for call-site signature
        // parity (test bodies still pass it) but no longer stored.
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind(did)
            .bind(format!("{}-handle", did.replace(':', "-")))
            .bind(chrono::Utc::now().to_rfc3339())
            .execute(db)
            .await
            .expect("seed actor");
        sqlx::query(
            "INSERT INTO plc_keys (did, last_operation_cid, atproto_signing_key) \
             VALUES ($1, $2, $3)",
        )
        .bind(did)
        .bind(Option::<String>::None)
        .bind(atproto)
        .execute(db)
        .await
        .expect("seed plc_keys");
    }

    async fn fresh_db() -> AnyPool {
        use std::sync::Once;
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("migrate");
        pool
    }

    #[tokio::test]
    async fn auth_headers_happy_path_mints_bearer_with_kid() {
        let db = fresh_db().await;
        let user_did = "did:plc:userarc12testxxxxxxxxx";
        let entryway_did = "did:web:entryway.test";
        // Distinct 32-byte hex.
        let atproto_key = "11".repeat(32);
        seed_plc_keys(&db, user_did, &"22".repeat(32), &atproto_key).await;

        let headers = entryway_auth_headers(&db, user_did, entryway_did, "com.atproto.server.getSession")
            .await
            .expect("mint");
        let auth = headers
            .get(axum::http::header::AUTHORIZATION)
            .expect("auth header present")
            .to_str()
            .unwrap();
        assert!(auth.starts_with("Bearer "));
        let jwt = auth.trim_start_matches("Bearer ");
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3, "JWT must be 3 segments");

        // Inspect header for alg/kid.
        let header_bytes = URL_SAFE_NO_PAD.decode(parts[0]).expect("header b64");
        let header_json: serde_json::Value =
            serde_json::from_slice(&header_bytes).expect("header json");
        assert_eq!(header_json["alg"], "ES256K");
        assert_eq!(header_json["kid"], AURORA_LOCAL_KID);

        // Inspect payload for iss/aud/lxm/exp~now+60.
        let claims_bytes = URL_SAFE_NO_PAD.decode(parts[1]).expect("claims b64");
        let claims_json: serde_json::Value =
            serde_json::from_slice(&claims_bytes).expect("claims json");
        assert_eq!(claims_json["iss"], user_did);
        assert_eq!(claims_json["aud"], entryway_did);
        assert_eq!(claims_json["lxm"], "com.atproto.server.getSession");
        let iat = claims_json["iat"].as_i64().unwrap();
        let exp = claims_json["exp"].as_i64().unwrap();
        assert_eq!(exp - iat, 60, "§5.3.5 TTL ≤60s");
        assert!(claims_json["jti"].is_string(), "jti must be present for replay tracking");
    }

    #[tokio::test]
    async fn auth_headers_unknown_account_returns_notfound() {
        let db = fresh_db().await;
        let err = entryway_auth_headers(
            &db,
            "did:plc:nosuchaccountxxxxxxxxxxx",
            "did:web:entryway.test",
            "com.atproto.server.getSession",
        )
        .await
        .expect_err("must fail");
        assert!(matches!(err, PdsError::NotFound(_)));
        let msg = err.to_string();
        assert!(msg.contains("UnknownAccount"));
    }

    #[tokio::test]
    async fn auth_headers_keynotfound_when_atproto_signing_key_empty() {
        let db = fresh_db().await;
        let user_did = "did:plc:legacyaccountxxxxxxxxx";
        // Legacy row: rotation_key populated, atproto_signing_key default-empty.
        seed_plc_keys(&db, user_did, &"33".repeat(32), "").await;

        let err = entryway_auth_headers(&db, user_did, "did:web:entryway.test", "lxm.test")
            .await
            .expect_err("must fail");
        assert!(matches!(err, PdsError::Authentication(_)));
        let msg = err.to_string();
        assert!(msg.contains("KeyNotFound"));
    }

    #[tokio::test]
    async fn auth_headers_remint_is_distinct_per_call() {
        let db = fresh_db().await;
        let user_did = "did:plc:remintaccountxxxxxxxxx";
        seed_plc_keys(&db, user_did, &"44".repeat(32), &"55".repeat(32)).await;

        let h1 = entryway_auth_headers(&db, user_did, "did:web:entryway.test", "lxm.x")
            .await
            .unwrap();
        let h2 = entryway_auth_headers(&db, user_did, "did:web:entryway.test", "lxm.x")
            .await
            .unwrap();
        let j1 = h1.get(axum::http::header::AUTHORIZATION).unwrap().to_str().unwrap();
        let j2 = h2.get(axum::http::header::AUTHORIZATION).unwrap().to_str().unwrap();
        // Per §5.3.5: re-mint per forward, fresh jti each time, so
        // the bytes differ.
        assert_ne!(j1, j2, "consecutive mints must differ (fresh jti)");
    }

    // ---------- entryway_passthru_headers tests (§5.4 Step 2.2) ----------

    fn h(name: &str, val: &str) -> (axum::http::HeaderName, HeaderValue) {
        (
            name.parse().unwrap(),
            HeaderValue::from_str(val).unwrap(),
        )
    }

    #[test]
    fn passthru_copies_authorization() {
        let mut incoming = HeaderMap::new();
        let (n, v) = h("authorization", "Bearer abc.def.ghi");
        incoming.insert(n, v);
        let out = entryway_passthru_headers(&incoming, None).expect("ok");
        assert_eq!(out.get("authorization").unwrap(), "Bearer abc.def.ghi");
    }

    #[test]
    fn passthru_rejects_dpop_scheme() {
        let mut incoming = HeaderMap::new();
        let (n, v) = h("authorization", "DPoP eyJ.eyJ.sig");
        incoming.insert(n, v);
        let err = entryway_passthru_headers(&incoming, None).expect_err("must reject");
        assert!(matches!(err, PdsError::Validation(_)));
        assert!(err.to_string().contains("DPoP requests cannot be proxied"));
    }

    #[test]
    fn passthru_rejects_dpop_header_alongside_bearer() {
        let mut incoming = HeaderMap::new();
        let (n, v) = h("authorization", "Bearer abc.def.ghi");
        incoming.insert(n, v);
        let (n2, v2) = h("dpop", "eyJ.proof.sig");
        incoming.insert(n2, v2);
        let err = entryway_passthru_headers(&incoming, None).expect_err("must reject");
        assert!(matches!(err, PdsError::Validation(_)));
    }

    #[test]
    fn passthru_preserves_existing_xff() {
        let mut incoming = HeaderMap::new();
        let (n, v) = h("x-forwarded-for", "203.0.113.7");
        incoming.insert(n, v);
        let out = entryway_passthru_headers(
            &incoming,
            Some("10.0.0.1:54321".parse().unwrap()),
        )
        .expect("ok");
        assert_eq!(
            out.get("x-forwarded-for").unwrap(),
            "203.0.113.7",
            "existing X-Forwarded-For from reverse proxy must be preserved verbatim"
        );
    }

    #[test]
    fn passthru_writes_remote_addr_when_no_xff() {
        let incoming = HeaderMap::new();
        let out = entryway_passthru_headers(
            &incoming,
            Some("10.0.0.1:54321".parse().unwrap()),
        )
        .expect("ok");
        assert_eq!(out.get("x-forwarded-for").unwrap(), "10.0.0.1");
    }

    #[test]
    fn passthru_empty_incoming_no_remote_addr_yields_empty_outgoing() {
        let incoming = HeaderMap::new();
        let out = entryway_passthru_headers(&incoming, None).expect("ok");
        assert!(out.is_empty());
    }

    #[test]
    fn passthru_drops_v4_1_set_headers_per_current_bsky_pds() {
        // Verify the deliberate scope narrowing: v4.1's enumerated set
        // (Content-Type / Accept-Language / User-Agent / Atproto-Proxy /
        // Idempotency-Key / X-Forwarded-Host) is NOT forwarded by the
        // current bsky-PDS pattern. Phase B integration testing surfaces
        // the gap if any of these is needed; §5.3.6 then gets revised
        // with concrete drivers.
        let mut incoming = HeaderMap::new();
        let (n, v) = h("authorization", "Bearer abc.def.ghi");
        incoming.insert(n, v);
        for (name, val) in [
            ("content-type", "application/json"),
            ("accept-language", "en-US"),
            ("user-agent", "test/1.0"),
            ("atproto-proxy", "did:web:test"),
            ("idempotency-key", "abc-123"),
            ("x-forwarded-host", "client.example"),
        ] {
            let (n, v) = h(name, val);
            incoming.insert(n, v);
        }
        let out = entryway_passthru_headers(&incoming, None).expect("ok");
        assert!(out.contains_key("authorization"));
        for dropped in [
            "content-type",
            "accept-language",
            "user-agent",
            "atproto-proxy",
            "idempotency-key",
            "x-forwarded-host",
        ] {
            assert!(
                !out.contains_key(dropped),
                "{} must NOT be forwarded per current bsky-PDS pattern",
                dropped
            );
        }
    }
}
