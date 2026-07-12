//! Aurora-owned DPoP proof construction (RFC 9449 §4.2).
//!
//! Phase 1 of the Aurora-owned admin OAuth arc (chainlink #439). This module
//! produces spec-compliant DPoP proof JWTs — critically including the §4.2
//! REQUIRED `exp` claim that upstream proto-blue-oauth v0.3.3 omits and that
//! Aurora's own strict validator ([`crate::federation::dpop`]) correctly
//! rejects. Owning proof construction here is what decouples the admin OAuth
//! ceremony from that upstream defect.
//!
//! Correctness is anchored to the validator, not merely to the RFC: the proofs
//! this builder emits round-trip through [`crate::federation::dpop::DPopVerifier`]
//! (see the tests), and the module deliberately reuses
//! [`crate::federation::dpop::compute_ath`] so the access-token-hash recipe is
//! byte-identical to what the validator recomputes.
//!
//! Consumed by [`super::admin::AdminOAuthClient`], which the admin OAuth
//! callback (`api::oauth_admin`) drives — so this builds every DPoP proof the
//! admin login ceremony presents to Aurora's own AS.

use crate::crypto::keypair::{Jwk, KeyPair};
use serde::Serialize;

/// DPoP proof lifetime, in seconds. RFC 9449 §11.1 calls for a short-lived
/// proof; 60s is the ecosystem norm and sits comfortably inside the window
/// Aurora's own validator tolerates (`validate_exp = true`, `leeway = 0`).
const DPOP_PROOF_LIFETIME_SECS: i64 = 60;

/// Errors raised while constructing a DPoP proof.
#[derive(Debug, thiserror::Error)]
pub enum DpopError {
    /// Deriving the PKCS#8 PEM or public JWK from the generated key failed.
    #[error("DPoP key material derivation failed: {0}")]
    KeyMaterial(String),
    /// The public JWK could not be embedded into the JWT header.
    #[error("DPoP proof header construction failed: {0}")]
    Header(String),
    /// ES256 signing of the proof JWT failed.
    #[error("DPoP proof signing failed: {0}")]
    Signing(String),
}

/// DPoP proof JWT claim set (RFC 9449 §4.2 / §4.3).
///
/// `nonce` and `ath` are omitted from the serialized JSON when absent
/// (`skip_serializing_if`) — an issuance proof carries neither, and emitting
/// them as `null` would diverge from what strict validators expect.
#[derive(Debug, Serialize)]
struct DpopProofClaims<'a> {
    /// Unique per proof; the validator's replay guard keys on this.
    jti: String,
    /// HTTP method, uppercased.
    htm: String,
    /// HTTP target URI, query + fragment stripped.
    htu: String,
    /// Issued-at (Unix seconds).
    iat: i64,
    /// Expiry (Unix seconds) — the claim upstream omits.
    exp: i64,
    /// Server-issued nonce, when the AS has demanded one.
    #[serde(skip_serializing_if = "Option::is_none")]
    nonce: Option<&'a str>,
    /// `base64url(SHA-256(access_token))`, when a token accompanies the proof.
    #[serde(skip_serializing_if = "Option::is_none")]
    ath: Option<String>,
}

/// Builds RFC 9449-compliant DPoP proof JWTs for a single OAuth ceremony.
///
/// Holds one ephemeral P-256 key for its whole lifetime: the AS binds the
/// issued tokens to this key, so the PAR, token-exchange, and refresh requests
/// within a ceremony MUST every one present a proof signed by the *same* key.
/// Construct one `DpopProofBuilder` per admin login flow and reuse it across
/// that flow's requests.
pub struct DpopProofBuilder {
    /// PKCS#8 PEM of the ephemeral P-256 private key, in the exact form
    /// [`jsonwebtoken::EncodingKey::from_ec_pem`] consumes. Never serialized
    /// into a proof; only the public half (below) reaches the wire.
    signing_key_pem: String,
    /// The corresponding PUBLIC JWK (`d` cleared), embedded in every proof
    /// header so the AS can bind tokens to this key. Guaranteed free of the
    /// private scalar — see [`DpopProofBuilder::new`].
    public_jwk: Jwk,
    /// Most recent server-issued `DPoP-Nonce`, if the AS has demanded one.
    /// Set via [`DpopProofBuilder::with_nonce`]; the OAuth client (Phase 2)
    /// owns the request/response round-trip that feeds it.
    nonce: Option<String>,
}

impl DpopProofBuilder {
    /// Generate a fresh ephemeral P-256 key and prepare a builder around it.
    ///
    /// Fallible because deriving the PKCS#8 PEM and public JWK from the key are
    /// themselves fallible operations; a freshly generated key never trips them
    /// in practice, but library code propagates the error rather than panicking.
    pub fn new() -> Result<Self, DpopError> {
        let keypair = KeyPair::generate();
        let signing_key_pem = keypair
            .private_key_pem()
            .map_err(|e| DpopError::KeyMaterial(e.to_string()))?;
        let mut public_jwk = keypair
            .public_key_jwk()
            .map_err(|e| DpopError::KeyMaterial(e.to_string()))?;
        // `public_key_jwk` already leaves `d` unset, but pin the invariant here:
        // the private scalar must never travel in a proof header. A future
        // refactor that accidentally returned a private JWK would be caught by
        // the `header_jwk_omits_private_scalar` test rather than leaking.
        public_jwk.d = None;
        Ok(Self {
            signing_key_pem,
            public_jwk,
            nonce: None,
        })
    }

    /// Record a server-issued DPoP nonce to embed in subsequent proofs.
    ///
    /// The AS signals it wants a nonce by returning a `DPoP-Nonce` header (and,
    /// on the first unprimed request, a `use_dpop_nonce` error); the client
    /// stores it here and rebuilds the proof.
    pub fn with_nonce(&mut self, nonce: String) {
        self.nonce = Some(nonce);
    }

    /// Build a signed DPoP proof JWT for one HTTP request (RFC 9449 §4.2).
    ///
    /// * `htm` — HTTP method; emitted uppercased (the canonical form the
    ///   validator compares case-insensitively).
    /// * `htu` — HTTP target URI; query and fragment are stripped to match the
    ///   canonicalization [`crate::federation::dpop`] performs before comparing.
    /// * `access_token` — when `Some`, adds the §4.3 `ath` binding
    ///   (`base64url(SHA-256(token))`), required on any proof that accompanies
    ///   an access token (refresh, resource requests). Pass `None` for the
    ///   token-issuance proofs (PAR, code exchange) where no token exists yet.
    pub fn build_proof(
        &self,
        htm: &str,
        htu: &str,
        access_token: Option<&str>,
    ) -> Result<String, DpopError> {
        use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};

        let now = chrono::Utc::now().timestamp();
        let claims = DpopProofClaims {
            jti: uuid::Uuid::new_v4().to_string(),
            htm: htm.to_uppercase(),
            htu: canonical_htu(htu),
            iat: now,
            exp: now + DPOP_PROOF_LIFETIME_SECS,
            nonce: self.nonce.as_deref(),
            ath: access_token.map(crate::federation::dpop::compute_ath),
        };

        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some("dpop+jwt".to_string());
        // `jsonwebtoken` carries its own JWK type in the header; bridge our
        // public JWK into it through serde_json (the same round-trip the
        // validator's own tests use — no dependency on that crate's internals).
        let jwk_value = serde_json::to_value(&self.public_jwk)
            .map_err(|e| DpopError::Header(e.to_string()))?;
        header.jwk = Some(
            serde_json::from_value(jwk_value).map_err(|e| DpopError::Header(e.to_string()))?,
        );

        let encoding_key = EncodingKey::from_ec_pem(self.signing_key_pem.as_bytes())
            .map_err(|e| DpopError::Signing(e.to_string()))?;

        encode(&header, &claims, &encoding_key).map_err(|e| DpopError::Signing(e.to_string()))
    }
}

/// Canonicalize a DPoP `htu`: strip fragment then query (RFC 9449 §4.2). This
/// mirrors the comparison [`crate::federation::dpop`] performs, so a proof this
/// builder emits for a given target matches the validator's expectation for the
/// same target regardless of query string.
fn canonical_htu(htu: &str) -> String {
    let no_fragment = htu.split('#').next().unwrap_or(htu);
    let no_query = no_fragment.split('?').next().unwrap_or(no_fragment);
    no_query.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Base64url-decode a JWT segment into JSON.
    fn decode_segment(seg: &str) -> serde_json::Value {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine;
        let bytes = URL_SAFE_NO_PAD.decode(seg).expect("valid base64url segment");
        serde_json::from_slice(&bytes).expect("segment is JSON")
    }

    /// Split a compact JWT into (header, claims) JSON values.
    fn split_jwt(jwt: &str) -> (serde_json::Value, serde_json::Value) {
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3, "compact JWT must have three segments");
        (decode_segment(parts[0]), decode_segment(parts[1]))
    }

    #[test]
    fn build_proof_includes_all_required_claims_and_header() {
        let builder = DpopProofBuilder::new().unwrap();
        let jwt = builder
            .build_proof("POST", "https://as.example/oauth/par", None)
            .unwrap();
        let (header, claims) = split_jwt(&jwt);

        assert_eq!(header["typ"], "dpop+jwt");
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["jwk"]["kty"], "EC");
        assert_eq!(header["jwk"]["crv"], "P-256");
        assert!(header["jwk"]["x"].is_string());
        assert!(header["jwk"]["y"].is_string());

        assert!(!claims["jti"].as_str().unwrap().is_empty());
        assert_eq!(claims["htm"], "POST");
        assert_eq!(claims["htu"], "https://as.example/oauth/par");
        assert!(claims["iat"].is_number());
        assert!(claims["exp"].is_number());
    }

    #[test]
    fn exp_is_iat_plus_lifetime() {
        let builder = DpopProofBuilder::new().unwrap();
        let jwt = builder
            .build_proof("GET", "https://as.example/x", None)
            .unwrap();
        let (_h, claims) = split_jwt(&jwt);
        let iat = claims["iat"].as_i64().unwrap();
        let exp = claims["exp"].as_i64().unwrap();
        assert_eq!(exp - iat, DPOP_PROOF_LIFETIME_SECS);
    }

    #[test]
    fn htm_is_uppercased() {
        let builder = DpopProofBuilder::new().unwrap();
        let jwt = builder
            .build_proof("post", "https://as.example/x", None)
            .unwrap();
        let (_h, claims) = split_jwt(&jwt);
        assert_eq!(claims["htm"], "POST");
    }

    #[test]
    fn htu_strips_query_and_fragment() {
        let builder = DpopProofBuilder::new().unwrap();
        let jwt = builder
            .build_proof(
                "GET",
                "https://as.example/authorize?client_id=x&state=y#frag",
                None,
            )
            .unwrap();
        let (_h, claims) = split_jwt(&jwt);
        assert_eq!(claims["htu"], "https://as.example/authorize");
    }

    #[test]
    fn nonce_absent_by_default_then_present_after_with_nonce() {
        let mut builder = DpopProofBuilder::new().unwrap();
        let jwt = builder
            .build_proof("POST", "https://as.example/token", None)
            .unwrap();
        let (_h, claims) = split_jwt(&jwt);
        assert!(claims.get("nonce").is_none());

        builder.with_nonce("server-nonce-abc".to_string());
        let jwt2 = builder
            .build_proof("POST", "https://as.example/token", None)
            .unwrap();
        let (_h2, claims2) = split_jwt(&jwt2);
        assert_eq!(claims2["nonce"], "server-nonce-abc");
    }

    #[test]
    fn ath_absent_without_token_and_correct_with_token() {
        let builder = DpopProofBuilder::new().unwrap();

        let no_token = builder
            .build_proof("POST", "https://as.example/x", None)
            .unwrap();
        let (_h, claims) = split_jwt(&no_token);
        assert!(claims.get("ath").is_none());

        let token = "access-token-xyz";
        let with_token = builder
            .build_proof("POST", "https://as.example/x", Some(token))
            .unwrap();
        let (_h2, claims2) = split_jwt(&with_token);
        // Must equal the validator's own recipe exactly.
        assert_eq!(claims2["ath"], crate::federation::dpop::compute_ath(token));
    }

    #[test]
    fn header_jwk_omits_private_scalar() {
        let builder = DpopProofBuilder::new().unwrap();
        let jwt = builder
            .build_proof("GET", "https://as.example/x", None)
            .unwrap();
        let (header, _claims) = split_jwt(&jwt);
        assert!(
            header["jwk"].get("d").is_none(),
            "public JWK in a DPoP proof header must never carry the private scalar"
        );
    }

    #[test]
    fn distinct_proofs_carry_distinct_jti() {
        let builder = DpopProofBuilder::new().unwrap();
        let (_h1, c1) = split_jwt(
            &builder
                .build_proof("GET", "https://as.example/x", None)
                .unwrap(),
        );
        let (_h2, c2) = split_jwt(
            &builder
                .build_proof("GET", "https://as.example/x", None)
                .unwrap(),
        );
        assert_ne!(c1["jti"], c2["jti"], "jti must be unique per proof");
    }

    // ---- Round-trip against Aurora's own strict DPoP validator ----
    //
    // The load-bearing tests: a proof this builder emits must satisfy the very
    // validator (`federation::dpop`) that rejects upstream proto-blue-oauth's
    // exp-less proofs. If these pass, the admin OAuth ceremony can present
    // Aurora-built proofs to Aurora's own AS.

    #[tokio::test]
    async fn proof_round_trips_through_validator_issuance_flow() {
        use crate::federation::dpop::{DPopNonceStore, DPopVerifier};
        use std::sync::Arc;

        let builder = DpopProofBuilder::new().unwrap();
        let htu = "https://as.example/oauth/token";
        let proof = builder.build_proof("POST", htu, None).unwrap();

        let verifier = DPopVerifier::new(Arc::new(DPopNonceStore::new()));
        let thumbprint = verifier
            .verify_dpop_proof(&proof, "POST", htu, None)
            .await
            .expect("Aurora-built issuance proof must verify against Aurora's validator");
        assert!(!thumbprint.is_empty());
    }

    #[tokio::test]
    async fn proof_round_trips_through_validator_resource_flow_with_ath() {
        use crate::federation::dpop::{compute_ath, DPopNonceStore, DPopVerifier};
        use std::sync::Arc;

        let builder = DpopProofBuilder::new().unwrap();
        let htu = "https://as.example/xrpc/resource";
        let token = "bound-access-token";
        let proof = builder.build_proof("POST", htu, Some(token)).unwrap();

        let verifier = DPopVerifier::new(Arc::new(DPopNonceStore::new()));
        verifier
            .verify_dpop_proof(&proof, "POST", htu, Some(&compute_ath(token)))
            .await
            .expect("Aurora-built ath-bound proof must verify against Aurora's validator");
    }

    #[tokio::test]
    async fn validator_rejects_replay_of_builder_proof() {
        // Presenting the same proof bytes twice trips the validator's jti
        // replay guard — confirming our jti reaches the claim the validator
        // keys on, and that a captured proof is single-use.
        use crate::federation::dpop::{DPopNonceStore, DPopVerifier};
        use std::sync::Arc;

        let builder = DpopProofBuilder::new().unwrap();
        let htu = "https://as.example/oauth/token";
        let proof = builder.build_proof("POST", htu, None).unwrap();

        let verifier = DPopVerifier::new(Arc::new(DPopNonceStore::new()));
        verifier
            .verify_dpop_proof(&proof, "POST", htu, None)
            .await
            .expect("first presentation accepted");
        let err = verifier
            .verify_dpop_proof(&proof, "POST", htu, None)
            .await
            .expect_err("replay must be rejected");
        assert!(format!("{err}").contains("replay"), "got: {err}");
    }
}
