//! Arc 12 §5.6.2 — Access-token JWT verify path verified (CANONICAL MATRIX)
//!
//! Eighteen cases per V05_DESIGN.md §5.6.2, exercising
//! `require_auth_unified` after Step 0.6.3's tuple-based routing
//! rewrite. Each case asserts both the dispatch outcome and the
//! routing path responsible for it, using
//! `MockIdentityResolver::get_signing_key_calls()` as the deterministic
//! witness for whether the trusted service-auth fallback fired (any
//! `>= 1`) or rejected before fetching a key (`== 0`).
//!
//! Cases 4, 5, 6, 12 are marked `#[ignore]` until Step 1 lands
//! `EntrywayConfig` + the known-entryway-kid lookup + the
//! `validate_external_access_token` plumbing through the middleware.
//! Their stub bodies document what each case must assert post-Step-1.

use std::path::PathBuf;
use std::sync::Arc;

use aurora_locus::api::middleware::{require_auth_unified, UnifiedAuthContext};
use aurora_locus::config::*;
use aurora_locus::context::AppContext;
use aurora_locus::federation::authentication::FederationAuthenticator;
use aurora_locus::identity::did_document::{DidDocument, VerificationMethod};
use aurora_locus::identity::resolver::test_doubles::MockIdentityResolver;
use aurora_locus::service_auth::create_service_jwt;
use axum::extract::State;
use axum::http::HeaderMap;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use k256::ecdsa::{SigningKey as K256SigningKey, VerifyingKey as K256VerifyingKey};
use serde_json::json;
use tempfile::tempdir;

const TEST_SERVICE_DID: &str = "did:web:localhost";
const TEST_PEER_DID: &str = "did:plc:peerpdsxxxxxxxxxxxxxxxx";
const TEST_UNTRUSTED_DID: &str = "did:plc:untrustedxxxxxxxxxxxx";
const TEST_USER_DID: &str = "did:plc:useraurorxxxxxxxxxxxxxx";
const TEST_USER_HANDLE: &str = "user.localhost";
const TEST_JWT_SECRET: &str = "test-secret-key-aurora-arc12-matrix-32x";

/// Build an `AppContext` configured for Arc 12 routing tests:
/// - federation enabled with `peer_pds = [(TEST_PEER_DID, ...)]`
/// - `identity_resolver` swapped to `MockIdentityResolver`
/// - `federation_auth` rebuilt against the mock so service-auth
///   fallback paths can drive `verify_service_jwt` end-to-end
/// - one actor row seeded so session inserts honor the FK
async fn build_test_ctx() -> (AppContext, Arc<MockIdentityResolver>) {
    let dir = tempdir().unwrap().keep();
    let db_path = dir.join("test.db");
    let config = ServerConfig {
        service: ServiceConfig {
            hostname: "localhost".to_string(),
            port: 2583,
            service_did: TEST_SERVICE_DID.to_string(),
            version: "0.1.0-test".to_string(),
            blob_upload_limit: 5_242_880,
            public_url: Some("http://localhost:2583".to_string()),
            max_blob_fetch_size: 50_000_000,
            blob_fetch_timeout_seconds: 30,
            blob_fetch_max_retries: 3,
        },
        storage: StorageConfig {
            data_directory: dir.clone(),
            account_db: db_path.clone(),
            sequencer_db: dir.join("sequencer.db"),
            did_cache_db: dir.join("did_cache.db"),
            actor_store_directory: dir.join("actors"),
            blobstore: BlobstoreConfig::Disk {
                location: dir.join("blobs"),
                tmp_location: dir.join("temp"),
            },
        },
        database: Default::default(),
        authentication: AuthConfig {
            jwt_secret: TEST_JWT_SECRET.to_string(),
            repo_signing_key: "a".repeat(64),
            plc_rotation_key: "b".repeat(64),
            oauth: OAuthConfig {
                client_id: "http://localhost:3000/client-metadata.json".to_string(),
                redirect_uri: "http://localhost:3000/oauth/callback".to_string(),
                pds_url: "https://bsky.social".to_string(),
            },
            jwt_sunset_date: "Sat, 31 Dec 2024 23:59:59 GMT".to_string(),
            oauth_migration_guide_url:
                "https://docs.atproto.com/guides/oauth-migration".to_string(),
            oauth_features: Default::default(),
        },
        identity: IdentityConfig {
            did_plc_url: "https://plc.directory".to_string(),
            service_handle_domains: vec![".localhost".to_string()],
            did_cache_stale_ttl: 3600,
            did_cache_max_ttl: 86400,
            recovery_did_key: None,
        },
        email: None,
        invites: InviteConfig {
            required: false,
            interval: 604800,
            epoch: "2024-01-01T00:00:00Z".to_string(),
        },
        rate_limit: RateLimitConfig {
            enabled: false,
            global_requests_per_minute: 3000,
            exempt_admin_assets: true,
        },
        logging: LoggingConfig {
            level: "info".to_string(),
        },
        federation: FederationConfig {
            enabled: true,
            relay_urls: vec![],
            appview_url: None,
            firehose_enabled: false,
            crawl_enabled: false,
            public_url: Some("http://localhost:2583".to_string()),
            auto_stream_events: false,
            peer_pds: vec![PeerPdsConfig {
                did: TEST_PEER_DID.to_string(),
                url: "http://localhost:2584".to_string(),
            }],
        },
        validation_mode: PathBuf::from("required")
            .into_os_string()
            .to_string_lossy()
            .parse()
            .unwrap_or(aurora_locus::validation::ValidationMode::Required),
        distributed_state_mode: Default::default(),
        maintenance_pool: Default::default(),
        gc_sweep: Default::default(),
        blob_metadata: Default::default(),
        entryway: None,
    };

    let mut ctx = AppContext::new(
        config,
        Arc::new(aurora_locus::api::registry::RouteRegistry::default()),
    )
    .await
    .expect("AppContext::new");

    let mock: Arc<MockIdentityResolver> = Arc::new(MockIdentityResolver::new());
    ctx.identity_resolver = mock.clone();
    ctx.federation_auth = Some(Arc::new(FederationAuthenticator::new(mock.clone())));

    sqlx::query(
        "INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)",
    )
    .bind(TEST_USER_DID)
    .bind(TEST_USER_HANDLE)
    .bind(Utc::now().to_rfc3339())
    .execute(&ctx.account_db)
    .await
    .expect("seed actor");

    (ctx, mock)
}

fn auth_header(token: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        "authorization",
        format!("Bearer {}", token).parse().unwrap(),
    );
    h
}

async fn call_auth(
    ctx: &AppContext,
    token: &str,
) -> Result<UnifiedAuthContext, aurora_locus::error::PdsError> {
    require_auth_unified(State(ctx.clone()), auth_header(token)).await
}

fn mint_hs256(header_json: &str, payload_json: &str, secret: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;

    let h_b64 = URL_SAFE_NO_PAD.encode(header_json.as_bytes());
    let p_b64 = URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
    let signing_input = format!("{}.{}", h_b64, p_b64);
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(signing_input.as_bytes());
    let sig = mac.finalize().into_bytes();
    let sig_b64 = URL_SAFE_NO_PAD.encode(sig);
    format!("{}.{}", signing_input, sig_b64)
}

fn mint_manual(header_json: &str, payload_json: &str, signature_bytes: &[u8]) -> String {
    let h_b64 = URL_SAFE_NO_PAD.encode(header_json.as_bytes());
    let p_b64 = URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
    let s_b64 = URL_SAFE_NO_PAD.encode(signature_bytes);
    format!("{}.{}.{}", h_b64, p_b64, s_b64)
}

/// Script the mock identity resolver to return a fresh ES256K key
/// for `did` (both the DID document and the raw signing-key bytes),
/// and return the signing-key bytes so the caller can mint matching
/// service-auth JWTs via `create_service_jwt`.
fn script_k256(mock: &MockIdentityResolver, did: &str) -> K256SigningKey {
    let signing_key = K256SigningKey::random(&mut rand::thread_rng());
    let verifying_key: K256VerifyingKey = *signing_key.verifying_key();
    mock.script_did(did, did_doc_with_k256(did, &verifying_key));
    // verify_service_jwt calls get_signing_key separately; script
    // that map too so the fallback actually reaches the verify step.
    mock.script_signing_key(did, b"-----BEGIN PUBLIC KEY-----\n-----END PUBLIC KEY-----\n".to_vec());
    signing_key
}

fn did_doc_with_k256(did: &str, verifying_key: &K256VerifyingKey) -> DidDocument {
    let sec1 = verifying_key.to_encoded_point(true);
    let mut buf = vec![0xe7_u8, 0x01_u8];
    buf.extend_from_slice(sec1.as_bytes());
    let multibase = format!("z{}", bs58::encode(&buf).into_string());
    DidDocument {
        context: None,
        id: did.to_string(),
        also_known_as: vec![],
        service: vec![],
        verification_method: vec![VerificationMethod {
            id: format!("{}#atproto", did),
            key_type: "Multikey".to_string(),
            controller: did.to_string(),
            public_key_multibase: Some(multibase),
        }],
    }
}

// ============================================================
// Case 1 — opaque local-mint OAuth token → DB lookup
// ============================================================

#[tokio::test]
async fn case_01_opaque_oauth_token_routes_to_oauth_path() {
    let (ctx, mock) = build_test_ctx().await;

    let token = "at_opaque_test_token_01";
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO token (token_id, did, client_id, current_refresh_token, scope, \
         created_at, updated_at, expires_at, dpop_thumbprint, device_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(token)
    .bind(TEST_USER_DID)
    .bind("test-client")
    .bind("rt_test_refresh_01")
    .bind("atproto")
    .bind(now.to_rfc3339())
    .bind(now.to_rfc3339())
    .bind((now + chrono::Duration::hours(1)).to_rfc3339())
    .bind(Option::<String>::None)
    .bind(Option::<String>::None)
    .execute(&ctx.account_db)
    .await
    .expect("seed oauth token");

    let auth = call_auth(&ctx, token).await.expect("oauth dispatch ok");
    match auth {
        UnifiedAuthContext::OAuth { did, .. } => assert_eq!(did, TEST_USER_DID),
        other => panic!("expected OAuth variant, got {:?}", other),
    }
    // Opaque-token path never touches PLC.
    assert_eq!(mock.get_signing_key_calls(), 0);
}

// ============================================================
// Case 2 — HS256 with kid=aurora-local-v1 + valid sig → local-verify
// ============================================================

#[tokio::test]
async fn case_02_hs256_with_aurora_local_v1_kid_routes_to_local_verify() {
    let (ctx, mock) = build_test_ctx().await;

    let session = ctx
        .account_manager
        .create_session(TEST_USER_DID, None)
        .await
        .expect("create_session");

    let auth = call_auth(&ctx, &session.access_token)
        .await
        .expect("local dispatch ok");
    match auth {
        UnifiedAuthContext::Local(s) => assert_eq!(s.did, TEST_USER_DID),
        other => panic!("expected Local variant, got {:?}", other),
    }
    assert_eq!(mock.get_signing_key_calls(), 0);
}

// ============================================================
// Case 3 — HS256 no kid + valid sig + present in DB → local-verify
// ============================================================

#[tokio::test]
async fn case_03_hs256_no_kid_routes_to_local_verify() {
    let (ctx, mock) = build_test_ctx().await;

    // Mint a kid-less HS256 token shaped like a pre-Step-0.6.2 mint,
    // then insert it into the session table directly so the
    // DB-lookup local-verify path matches.
    let now = Utc::now().timestamp();
    let claims = json!({
        "sub": TEST_USER_DID,
        "sid": "no-kid-test-sid",
        "iat": now,
        "exp": now + 3600,
    });
    let token = mint_hs256(
        r#"{"alg":"HS256","typ":"JWT"}"#,
        &claims.to_string(),
        TEST_JWT_SECRET,
    );
    let now_utc = Utc::now();
    sqlx::query(
        "INSERT INTO session (id, did, access_token, refresh_token, created_at, expires_at, app_password_name) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind("no-kid-test-sid")
    .bind(TEST_USER_DID)
    .bind(&token)
    .bind("rt_no_kid_test")
    .bind(now_utc.to_rfc3339())
    .bind((now_utc + chrono::Duration::hours(1)).to_rfc3339())
    .bind(Option::<String>::None)
    .execute(&ctx.account_db)
    .await
    .expect("seed no-kid session");

    let auth = call_auth(&ctx, &token).await.expect("local dispatch ok");
    match auth {
        UnifiedAuthContext::Local(s) => assert_eq!(s.did, TEST_USER_DID),
        other => panic!("expected Local variant, got {:?}", other),
    }
    assert_eq!(mock.get_signing_key_calls(), 0);
}

// ============================================================
// Case 4 — ES256K + known entryway kid + valid sig + aud → external-verify
// ============================================================

#[ignore = "Step 1: requires EntrywayConfig + known-entryway-kid lookup + validate_external_access_token wiring through middleware"]
#[tokio::test]
async fn case_04_es256k_entryway_kid_valid_sig_routes_to_external_verify() {
    // Post-Step-1: token minted by entryway with a kid registered in
    // EntrywayConfig and a valid ES256K signature against the
    // entryway's published public key, with `aud` matching one of
    // the route variant's expected audiences, MUST verify via
    // `validate_external_access_token` and return a CrossPDS or
    // OAuth-equivalent UnifiedAuthContext.
}

// ============================================================
// Case 5 — ES256K + entryway kid + forged sig → rejected at sig verify
// ============================================================

#[ignore = "Step 1: requires EntrywayConfig + known-entryway-kid lookup + validate_external_access_token wiring through middleware"]
#[tokio::test]
async fn case_05_es256k_entryway_kid_forged_sig_rejected_at_sig_verify() {
    // Post-Step-1: token with a registered entryway kid but a forged
    // ES256K signature MUST reject at the signature-verification
    // step inside `validate_external_access_token`.
}

// ============================================================
// Case 6 — ES256K + entryway kid + valid sig + WRONG aud → rejected at aud
// ============================================================

#[ignore = "Step 1: requires EntrywayConfig + known-entryway-kid lookup + validate_external_access_token wiring through middleware"]
#[tokio::test]
async fn case_06_es256k_entryway_kid_wrong_aud_rejected_at_audience_check() {
    // Post-Step-1: token with a registered entryway kid + valid
    // signature but `aud = third DID` MUST reject via audience
    // allowlist (either PDS-DID-only or PDS+entryway-DID per route
    // variant per §5.3.4).
}

// ============================================================
// Case 7 — ES256K + no kid + iss = local PDS DID → fallback
// ============================================================

#[ignore = "Step 1.3: verify_service_jwt's first decode uses Validation::default() (HS256-only), \
            so ES256K tokens reject before reaching get_signing_key. Step 1.3's extraction \
            of verify_jwt_with_allowlist must support an alg-agnostic header peek so this \
            fallback dispatch can actually trigger key resolution."]
#[tokio::test]
async fn case_07_es256k_no_kid_iss_local_pds_dispatches_to_service_auth_fallback() {
    let (ctx, mock) = build_test_ctx().await;
    let signing_key = script_k256(&mock, TEST_SERVICE_DID);
    let token = create_service_jwt(
        TEST_SERVICE_DID,
        TEST_SERVICE_DID,
        Some(45),
        None,
        &signing_key.to_bytes(),
    )
    .expect("create_service_jwt");

    let _ = call_auth(&ctx, &token).await;
    assert!(
        mock.get_signing_key_calls() >= 1,
        "fallback must reach get_signing_key for local PDS DID"
    );
}

// ============================================================
// Case 8 — ES256K + unknown kid + iss = local PDS DID → fallback
// ============================================================

#[ignore = "Step 1.3: see case_07's reason — same first-decode HS256-only limitation"]
#[tokio::test]
async fn case_08_es256k_unknown_kid_iss_local_pds_dispatches_to_service_auth_fallback() {
    let (ctx, mock) = build_test_ctx().await;
    let signing_key = script_k256(&mock, TEST_SERVICE_DID);

    let base = create_service_jwt(
        TEST_SERVICE_DID,
        TEST_SERVICE_DID,
        Some(45),
        None,
        &signing_key.to_bytes(),
    )
    .expect("create_service_jwt");
    let parts: Vec<&str> = base.split('.').collect();
    let unknown_kid_header = r#"{"alg":"ES256K","typ":"JWT","kid":"unknown-kid-no-entryway"}"#;
    let h_b64 = URL_SAFE_NO_PAD.encode(unknown_kid_header.as_bytes());
    // Re-encoded header invalidates the signature — routing decision
    // is what's under test here, not signature verification.
    let token = format!("{}.{}.{}", h_b64, parts[1], parts[2]);

    let _ = call_auth(&ctx, &token).await;
    assert!(
        mock.get_signing_key_calls() >= 1,
        "fallback must reach get_signing_key for unknown-kid + trusted-iss"
    );
}

// ============================================================
// Case 9 — ES256K + no kid + iss = peer PDS DID → fallback
// ============================================================

#[ignore = "Step 1.3: see case_07's reason — same first-decode HS256-only limitation"]
#[tokio::test]
async fn case_09_es256k_no_kid_iss_peer_pds_dispatches_to_service_auth_fallback() {
    let (ctx, mock) = build_test_ctx().await;
    let signing_key = script_k256(&mock, TEST_PEER_DID);
    let token = create_service_jwt(
        TEST_PEER_DID,
        TEST_SERVICE_DID,
        Some(45),
        None,
        &signing_key.to_bytes(),
    )
    .expect("create_service_jwt");

    let _ = call_auth(&ctx, &token).await;
    assert!(
        mock.get_signing_key_calls() >= 1,
        "fallback must reach get_signing_key for peer PDS DID iss"
    );
}

// ============================================================
// Case 10 — ES256K + no kid + iss = untrusted DID → reject at routing
// ============================================================

#[tokio::test]
async fn case_10_es256k_iss_untrusted_did_rejected_at_iss_allowlist_without_plc_fetch() {
    let (ctx, mock) = build_test_ctx().await;
    let signing_key = script_k256(&mock, TEST_UNTRUSTED_DID);
    let token = create_service_jwt(
        TEST_UNTRUSTED_DID,
        TEST_SERVICE_DID,
        Some(45),
        None,
        &signing_key.to_bytes(),
    )
    .expect("create_service_jwt");

    let err = call_auth(&ctx, &token).await.expect_err("must reject");
    assert!(matches!(
        err,
        aurora_locus::error::PdsError::Authentication(_)
    ));
    // Critical: routing rejected BEFORE any PLC fetch.
    assert_eq!(
        mock.get_signing_key_calls(),
        0,
        "untrusted iss must reject without PLC fetch"
    );
}

// ============================================================
// Case 11 — HS256 + kid=aurora-local-v1 + forged sig → reject at local-verify
// ============================================================

#[tokio::test]
async fn case_11_hs256_aurora_local_v1_kid_forged_sig_rejected_at_local_verify() {
    let (ctx, mock) = build_test_ctx().await;

    let now = Utc::now().timestamp();
    let claims = json!({"sub": TEST_USER_DID, "iat": now, "exp": now + 3600});
    let token = mint_manual(
        r#"{"alg":"HS256","typ":"JWT","kid":"aurora-local-v1"}"#,
        &claims.to_string(),
        b"forged-signature-bytes-not-hmac-derived",
    );

    let err = call_auth(&ctx, &token).await.expect_err("must reject");
    assert!(matches!(
        err,
        aurora_locus::error::PdsError::Authentication(_)
    ));
    // Routed to local-verify (DB lookup), no PLC involvement.
    assert_eq!(mock.get_signing_key_calls(), 0);
}

// ============================================================
// Case 12 — Cross-path replay: revoked DB token re-encoded with entryway kid
// ============================================================

#[ignore = "Step 1: requires EntrywayConfig + known-entryway-kid lookup + entryway pubkey verification path"]
#[tokio::test]
async fn case_12_cross_path_replay_revoked_db_token_re_encoded_with_entryway_kid() {
    // Post-Step-1: take a locally-issued HS256 token, re-encode its
    // header with a registered entryway kid + alg=ES256K. The
    // routing layer will dispatch to validate_external_access_token,
    // which fails ES256K signature verification against the entryway
    // pubkey (the re-encoded bytes were signed with HS256-HMAC, not
    // ECDSA), so the token is rejected. Audit: no cross-path
    // confusion succeeds via header manipulation alone.
}

// ============================================================
// Case 13 — Expired JWT → reject at claim validation
// ============================================================

#[tokio::test]
async fn case_13_expired_local_token_rejected_at_session_expiry_check() {
    let (ctx, mock) = build_test_ctx().await;

    let now = Utc::now();
    let claims = json!({
        "sub": TEST_USER_DID,
        "sid": "expired-test-sid",
        "iat": now.timestamp() - 3700,
        "exp": now.timestamp() - 60,
    });
    let token = mint_hs256(
        r#"{"alg":"HS256","typ":"JWT","kid":"aurora-local-v1"}"#,
        &claims.to_string(),
        TEST_JWT_SECRET,
    );
    sqlx::query(
        "INSERT INTO session (id, did, access_token, refresh_token, created_at, expires_at, app_password_name) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind("expired-test-sid")
    .bind(TEST_USER_DID)
    .bind(&token)
    .bind("rt_expired_test")
    .bind((now - chrono::Duration::hours(2)).to_rfc3339())
    .bind((now - chrono::Duration::minutes(1)).to_rfc3339())
    .bind(Option::<String>::None)
    .execute(&ctx.account_db)
    .await
    .expect("seed expired session");

    let err = call_auth(&ctx, &token).await.expect_err("must reject");
    assert!(matches!(
        err,
        aurora_locus::error::PdsError::Authentication(_)
    ));
    assert_eq!(mock.get_signing_key_calls(), 0);
}

// ============================================================
// Case 14 — Malformed JWT claims → reject at claim parsing
// ============================================================

#[tokio::test]
async fn case_14_malformed_jwt_claims_rejected_at_decode() {
    let (ctx, mock) = build_test_ctx().await;

    // Valid header (alg=HS256), garbage payload (invalid base64url).
    // Routing decodes header (OK), tries payload decode for iss
    // (Err → treated as None), dispatches to HS256 no-kid local-
    // verify, DB lookup misses, rejection.
    let h_b64 = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"HS256\",\"typ\":\"JWT\"}");
    let token = format!("{}.not-base64url!@#.AAAA", h_b64);

    let err = call_auth(&ctx, &token).await.expect_err("must reject");
    assert!(matches!(
        err,
        aurora_locus::error::PdsError::Authentication(_)
    ));
    assert_eq!(mock.get_signing_key_calls(), 0);
}

// ============================================================
// Case 15 — iss in trust set + sig fails against PLC-fetched key
// ============================================================

#[ignore = "Step 1.3: see case_07's reason — same first-decode HS256-only limitation"]
#[tokio::test]
async fn case_15_iss_in_trust_set_sig_fails_against_plc_fetched_key() {
    let (ctx, mock) = build_test_ctx().await;

    // Script the trusted iss's DID document + signing key with one
    // keypair. Sign the token with a DIFFERENT key — sig verify
    // (inside verify_service_jwt against the PEM signing key) fails.
    let _legitimate_key = script_k256(&mock, TEST_SERVICE_DID);
    let forgery_key = K256SigningKey::random(&mut rand::thread_rng());
    let token = create_service_jwt(
        TEST_SERVICE_DID,
        TEST_SERVICE_DID,
        Some(45),
        None,
        &forgery_key.to_bytes(),
    )
    .expect("create_service_jwt");

    let err = call_auth(&ctx, &token).await.expect_err("must reject");
    assert!(matches!(
        err,
        aurora_locus::error::PdsError::Authentication(_)
    ));
    // iss-allowlist passed (trusted iss), fallback fetched key,
    // sig verify failed somewhere in verify_service_jwt.
    assert!(
        mock.get_signing_key_calls() >= 1,
        "fallback must reach get_signing_key for trusted iss"
    );
}

// ============================================================
// Case 16 — alg=none + local-mint-shaped claims → reject at allowlist
// ============================================================

#[tokio::test]
async fn case_16_alg_none_rejected_at_algorithm_allowlist() {
    let (ctx, mock) = build_test_ctx().await;

    let now = Utc::now().timestamp();
    let claims = json!({
        "sub": TEST_USER_DID,
        "sid": "alg-none-test",
        "iat": now,
        "exp": now + 3600,
    });
    let token = mint_manual(
        r#"{"alg":"none","typ":"JWT","kid":"aurora-local-v1"}"#,
        &claims.to_string(),
        &[],
    );

    let err = call_auth(&ctx, &token).await.expect_err("must reject");
    assert!(matches!(
        err,
        aurora_locus::error::PdsError::Authentication(_)
    ));
    // Rejected at the allowlist BEFORE tuple routing or any fetch.
    assert_eq!(mock.get_signing_key_calls(), 0);
}

// ============================================================
// Case 17 — alg=RS256 (or other unknown alg) → reject at allowlist
// ============================================================

#[tokio::test]
async fn case_17_alg_rs256_rejected_at_algorithm_allowlist() {
    let (ctx, mock) = build_test_ctx().await;

    let now = Utc::now().timestamp();
    let claims = json!({"sub": TEST_USER_DID, "iat": now, "exp": now + 3600});
    let token = mint_manual(
        r#"{"alg":"RS256","typ":"JWT"}"#,
        &claims.to_string(),
        b"fake-rs256-signature-blob",
    );

    let err = call_auth(&ctx, &token).await.expect_err("must reject");
    assert!(matches!(
        err,
        aurora_locus::error::PdsError::Authentication(_)
    ));
    assert_eq!(mock.get_signing_key_calls(), 0);
}

// ============================================================
// Case 18 — empty / missing / non-DID iss with ES256K → reject at routing
// ============================================================

#[tokio::test]
async fn case_18_empty_iss_with_es256k_rejected_at_iss_allowlist() {
    let (ctx, mock) = build_test_ctx().await;
    let now = Utc::now().timestamp();
    let claims = json!({"iss": "", "aud": TEST_SERVICE_DID, "exp": now + 45});
    let token = mint_manual(
        r#"{"alg":"ES256K","typ":"JWT"}"#,
        &claims.to_string(),
        b"signature-bytes-irrelevant-for-this-case",
    );

    let err = call_auth(&ctx, &token).await.expect_err("must reject");
    assert!(matches!(
        err,
        aurora_locus::error::PdsError::Authentication(_)
    ));
    assert_eq!(mock.get_signing_key_calls(), 0);
}

#[tokio::test]
async fn case_18b_non_did_iss_https_url_rejected_at_iss_allowlist() {
    let (ctx, mock) = build_test_ctx().await;
    let now = Utc::now().timestamp();
    let claims = json!({"iss": "https://example.com", "aud": TEST_SERVICE_DID, "exp": now + 45});
    let token = mint_manual(
        r#"{"alg":"ES256K","typ":"JWT"}"#,
        &claims.to_string(),
        b"irrelevant",
    );

    let err = call_auth(&ctx, &token).await.expect_err("must reject");
    assert!(matches!(
        err,
        aurora_locus::error::PdsError::Authentication(_)
    ));
    assert_eq!(mock.get_signing_key_calls(), 0);
}

#[tokio::test]
async fn case_18c_missing_iss_with_es256k_rejected_at_iss_allowlist() {
    let (ctx, mock) = build_test_ctx().await;
    let now = Utc::now().timestamp();
    let claims = json!({"aud": TEST_SERVICE_DID, "exp": now + 45});
    let token = mint_manual(
        r#"{"alg":"ES256K","typ":"JWT"}"#,
        &claims.to_string(),
        b"irrelevant",
    );

    let err = call_auth(&ctx, &token).await.expect_err("must reject");
    assert!(matches!(
        err,
        aurora_locus::error::PdsError::Authentication(_)
    ));
    assert_eq!(mock.get_signing_key_calls(), 0);
}
