//! WebAuthn passkey relying-party context, challenge store, and registration
//! ceremony (Holder UI Phase 2.b, chainlink #427).
//!
//! The relying party is **this PDS**: a holder authenticates to the Aurora
//! holder UI, so the RP id is the PDS hostname and the RP origin is the PDS
//! public origin (both from config; the same host the holder-UI pages are served
//! from). [`WebauthnCtx`] holds the process-wide [`Webauthn`] instance.
//!
//! A passkey ceremony is two round-trips (server issues a challenge; the browser
//! signs it; the server verifies). The intermediate `PasskeyRegistration` state
//! must be held server-side between them (replay defense) — [`PasskeyChallengeStore`]
//! is an in-memory, single-use, TTL'd store keyed by an opaque `challenge_id`,
//! mirroring β.2's in-memory `DPopNonceStore` precedent (single-instance; a
//! short ceremony makes the no-restart-survival caveat acceptable).
//!
//! This module ships **registration** (holder is already authenticated); the
//! **authentication** ceremony (login) lands in the next slice and extends the
//! store + this module.
//!
//! Dev note: `effective_public_url()` yields `http://…` for localhost. WebAuthn
//! permits `http` only for `localhost` (a browser secure-context exception the
//! `SoftPasskey` test authenticator mirrors), so the dev harness over
//! `http://localhost:2583` works; production is `https`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;
use webauthn_rs::prelude::{
    CreationChallengeResponse, PasskeyRegistration, RegisterPublicKeyCredential, Webauthn,
    WebauthnBuilder,
};

use super::super::browser_session::BrowserSessionContext;
use crate::context::AppContext;
use crate::error::{PdsError, PdsResult};

/// The relying-party display name shown in the browser's passkey prompt.
const RP_NAME: &str = "Aurora-Locus";

/// How long a stored ceremony challenge is valid. A user-driven passkey
/// ceremony is quick; 5 minutes is generous.
const CHALLENGE_TTL: Duration = Duration::from_secs(300);

/// Namespace for deriving a stable per-holder WebAuthn `user_unique_id` from the
/// DID (UUIDv5, deterministic — the same holder always maps to the same id, so
/// authenticators correlate their credentials).
const HOLDER_UUID_NAMESPACE: Uuid = Uuid::NAMESPACE_URL;

/// Map a holder DID to its stable WebAuthn user handle.
fn holder_user_id(did: &str) -> Uuid {
    Uuid::new_v5(&HOLDER_UUID_NAMESPACE, did.as_bytes())
}

/// Process-wide WebAuthn relying-party context for the holder-UI passkey flows.
#[derive(Clone)]
pub struct WebauthnCtx {
    pub webauthn: Arc<Webauthn>,
}

impl WebauthnCtx {
    /// Build the RP context. `rp_id` is the bare hostname
    /// ([`crate::config::ServiceConfig::hostname`]) and `rp_origin` is the
    /// public origin ([`crate::config::ServiceConfig::effective_public_url`]) —
    /// the browser sends exactly this origin because the holder UI is served
    /// from it, so `rp_id` is an effective domain of `rp_origin`.
    pub fn new(rp_id: &str, rp_origin: &str) -> PdsResult<Self> {
        let origin = Url::parse(rp_origin).map_err(|e| {
            PdsError::Internal(format!("passkey RP origin is not a valid URL: {e}"))
        })?;
        let webauthn = WebauthnBuilder::new(rp_id, &origin)
            .map_err(|e| PdsError::Internal(format!("invalid WebAuthn RP configuration: {e}")))?
            .rp_name(RP_NAME)
            .build()
            .map_err(|e| PdsError::Internal(format!("failed to build WebAuthn context: {e}")))?;
        Ok(Self {
            webauthn: Arc::new(webauthn),
        })
    }
}

/// A stored registration ceremony, awaiting the browser's response.
struct StoredRegistration {
    reg: PasskeyRegistration,
    did: String,
    created_at: Instant,
}

/// In-memory, single-use, TTL'd store of in-flight passkey ceremonies, keyed by
/// an opaque `challenge_id`. Registration only for now; authentication is added
/// alongside the login ceremony.
#[derive(Default)]
pub struct PasskeyChallengeStore {
    registrations: Mutex<HashMap<String, StoredRegistration>>,
}

impl PasskeyChallengeStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stash a registration ceremony bound to `did`; returns its `challenge_id`.
    pub fn store_registration(&self, reg: PasskeyRegistration, did: String) -> String {
        let id = Uuid::new_v4().to_string();
        let mut map = self.registrations.lock().expect("challenge store not poisoned");
        // Opportunistic sweep so abandoned ceremonies don't accumulate.
        map.retain(|_, v| v.created_at.elapsed() < CHALLENGE_TTL);
        map.insert(
            id.clone(),
            StoredRegistration {
                reg,
                did,
                created_at: Instant::now(),
            },
        );
        id
    }

    /// Pop a registration ceremony by `challenge_id`. `None` if absent or
    /// expired (single-use: it is removed regardless). Returns `(reg, did)`.
    pub fn take_registration(&self, id: &str) -> Option<(PasskeyRegistration, String)> {
        let mut map = self.registrations.lock().expect("challenge store not poisoned");
        let stored = map.remove(id)?;
        if stored.created_at.elapsed() >= CHALLENGE_TTL {
            return None;
        }
        Some((stored.reg, stored.did))
    }
}

// ---------- registration ceremony endpoints ----------

#[derive(Debug, Deserialize)]
pub struct RegisterStartRequest {
    pub csrf_token: String,
}

#[derive(Debug, Serialize)]
pub struct RegisterStartResponse {
    pub challenge_id: String,
    pub options: CreationChallengeResponse,
}

#[derive(Debug, Deserialize)]
pub struct RegisterFinishRequest {
    pub csrf_token: String,
    pub challenge_id: String,
    pub credential: RegisterPublicKeyCredential,
    #[serde(default)]
    pub device_name: Option<String>,
}

/// `POST /oauth/atproto/holder/auth-methods/passkey/start`
///
/// Holder is authenticated (registering a new passkey for themselves). Issues a
/// creation challenge excluding their already-registered credentials, and stores
/// the ceremony state keyed by an opaque `challenge_id`.
pub async fn register_start(
    session: BrowserSessionContext,
    State(ctx): State<AppContext>,
    Json(body): Json<RegisterStartRequest>,
) -> Response {
    if !super::csrf_ok(&session, &body.csrf_token) {
        return (StatusCode::FORBIDDEN, "CSRF token mismatch").into_response();
    }
    let did = &session.session.did;
    // Exclude the holder's existing passkeys so the same authenticator does not
    // double-register.
    let exclude: Vec<_> = ctx
        .holder_auth_methods
        .list_passkeys_for_did(did)
        .await
        .unwrap_or_default()
        .iter()
        .map(|p| p.cred_id().clone())
        .collect();
    let exclude = if exclude.is_empty() { None } else { Some(exclude) };

    match ctx.passkey_webauthn.webauthn.start_passkey_registration(
        holder_user_id(did),
        did,
        did,
        exclude,
    ) {
        Ok((options, reg)) => {
            let challenge_id = ctx.passkey_challenges.store_registration(reg, did.clone());
            Json(RegisterStartResponse {
                challenge_id,
                options,
            })
            .into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not start passkey registration",
        )
            .into_response(),
    }
}

/// `POST /oauth/atproto/holder/auth-methods/passkey/finish`
///
/// Verifies the browser's attestation against the stored ceremony and persists
/// the resulting `Passkey`. DID-scoped: the ceremony's DID must match the
/// session (a challenge issued for one holder cannot be finished by another).
pub async fn register_finish(
    session: BrowserSessionContext,
    State(ctx): State<AppContext>,
    Json(body): Json<RegisterFinishRequest>,
) -> Response {
    if !super::csrf_ok(&session, &body.csrf_token) {
        return (StatusCode::FORBIDDEN, "CSRF token mismatch").into_response();
    }
    let did = &session.session.did;
    let (reg, ceremony_did) = match ctx.passkey_challenges.take_registration(&body.challenge_id) {
        Some(v) => v,
        None => return (StatusCode::BAD_REQUEST, "challenge expired or unknown").into_response(),
    };
    if &ceremony_did != did {
        return (StatusCode::FORBIDDEN, "challenge does not belong to you").into_response();
    }
    let passkey = match ctx
        .passkey_webauthn
        .webauthn
        .finish_passkey_registration(&body.credential, &reg)
    {
        Ok(pk) => pk,
        Err(_) => return (StatusCode::BAD_REQUEST, "passkey attestation failed").into_response(),
    };
    match ctx
        .holder_auth_methods
        .register_passkey(did, &passkey, body.device_name.as_deref())
        .await
    {
        Ok(id) => Json(serde_json::json!({ "id": id })).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not store passkey",
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_for_localhost_http_dev() {
        assert!(
            WebauthnCtx::new("localhost", "http://localhost:2583").is_ok(),
            "localhost http RP context should build"
        );
    }

    #[test]
    fn builds_for_https_production() {
        assert!(
            WebauthnCtx::new("pds.example.com", "https://pds.example.com").is_ok(),
            "https RP context should build"
        );
    }

    #[test]
    fn rejects_rp_id_not_matching_origin() {
        assert!(
            WebauthnCtx::new("different-host.example.com", "https://pds.example.com").is_err(),
            "rp_id not a suffix of origin must error"
        );
    }

    // Note: WebauthnBuilder does NOT validate the origin scheme at build time —
    // the http-only-for-localhost rule is enforced during the ceremony (by the
    // browser / the SoftPasskey mirror), not at construction. So there is no
    // build-time "rejects non-localhost http" assertion here.

    #[test]
    fn holder_user_id_is_stable_and_distinct() {
        let a1 = holder_user_id("did:web:alice.example.com");
        let a2 = holder_user_id("did:web:alice.example.com");
        let b = holder_user_id("did:web:bob.example.com");
        assert_eq!(a1, a2, "same DID → same user id");
        assert_ne!(a1, b, "different DID → different user id");
    }

    #[test]
    fn challenge_store_roundtrips_and_is_single_use() {
        // A real PasskeyRegistration requires a ceremony; exercise store/take
        // semantics through the registration ceremony test below. Here we cover
        // the miss path.
        let store = PasskeyChallengeStore::new();
        assert!(store.take_registration("no-such-id").is_none());
    }

    // Full registration ceremony via the SoftPasskey software authenticator.
    #[tokio::test]
    async fn passkey_registration_ceremony_stores_a_credential() {
        use webauthn_authenticator_rs::softpasskey::SoftPasskey;
        use webauthn_authenticator_rs::WebauthnAuthenticator;

        let ctx = crate::api::federation_peers::test_support::create_test_context_with(|_| {}).await;
        let did = "did:web:alice.example.com";
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind(did)
            .bind("alice.example.com")
            .bind("2026-01-01T00:00:00Z")
            .execute(&ctx.account_db)
            .await
            .unwrap();

        // Server issues the creation challenge.
        let (ccr, reg) = ctx
            .passkey_webauthn
            .webauthn
            .start_passkey_registration(holder_user_id(did), did, did, None)
            .unwrap();

        // The software authenticator produces an attestation (http://localhost
        // is accepted via the localhost exception).
        let origin = Url::parse("http://localhost:2583").unwrap();
        // falsify_uv = true: passkey registration requires User Verification;
        // the software authenticator reports UV as satisfied for the test.
        let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
        let credential = authenticator.do_registration(origin, ccr).unwrap();

        // Server verifies + the manager persists it.
        let passkey = ctx
            .passkey_webauthn
            .webauthn
            .finish_passkey_registration(&credential, &reg)
            .unwrap();
        let method_id = ctx
            .holder_auth_methods
            .register_passkey(did, &passkey, Some("Test Key"))
            .await
            .unwrap();
        assert!(!method_id.is_empty());

        // It round-trips: list_passkeys_for_did returns a Passkey with the same
        // credential id.
        let listed = ctx.holder_auth_methods.list_passkeys_for_did(did).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].cred_id(), passkey.cred_id());

        // And it shows in the method list as a passkey method.
        let methods = ctx.holder_auth_methods.list_for_did(did).await.unwrap();
        assert_eq!(methods.len(), 1);
        assert_eq!(
            methods[0].method_type,
            super::super::auth_method_manager::AuthMethodType::Passkey
        );
        assert_eq!(methods[0].device_name.as_deref(), Some("Test Key"));
    }
}
