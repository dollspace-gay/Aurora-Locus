//! OAuth 2.1 Implementation for ATProto
//!
//! This module implements the OAuth 2.1 specification with ATProto extensions:
//! - DPoP (Demonstrating Proof-of-Possession) token binding
//! - PKCE (Proof Key for Code Exchange) authorization flow
//! - Multi-device support with device management
//! - Refresh token rotation with replay detection
//!
//! References:
//! - https://datatracker.ietf.org/doc/html/draft-ietf-oauth-v2-1
//! - https://atproto.com/specs/oauth

pub mod atproto;
pub mod authorize;
pub mod client;
pub mod consent;
pub mod device;
pub mod flow_state_adapter;
pub mod models;
pub mod scope;
pub mod token;
pub mod token_rotation;

pub use flow_state_adapter::OAuthFlowStateAdapter;

pub use authorize::authorize;
pub use client::ClientManager;
pub use consent::{consent_screen, deny_authorization, grant_authorization};
pub use device::DeviceManager;
// Re-exports surface scope helpers at the canonical `oauth::` path. Some
// of them (lexicon_to_scope / require_all_scopes / require_any_scope) are
// only consumed by integration tests today; rust's unused_imports lint
// fires for pub-use of unused-internally items in lib-only builds, so
// silence it here. Removing the allow once a non-test caller appears.
#[allow(unused_imports)]
pub use scope::{
    enforce_namespace_scope, lexicon_to_scope, require_all_scopes, require_any_scope,
    require_scope, required_scopes_for_path, AtProtoScope, ScopeSet,
};
pub use token::token_endpoint;

/// SHA-256 hex digest of an OAuth bearer, used for storage and lookup.
///
/// Phase β.1 / R1 F-3.1: the raw bearer is returned to the client but never
/// persisted; only this digest lands in `token.access_token_hash` and serves
/// as the `validate_oauth_token` lookup key. SHA-256 (not Argon2id) is the
/// correct primitive here — bearers are high-entropy random tokens, not human
/// passwords, validation runs on every XRPC, and the token's own randomness
/// defeats brute-forcing the digest. Keeping the bearer off disk means a DB
/// compromise yields no usable credential.
pub(crate) fn access_token_hash(bearer: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bearer.as_bytes());
    hex::encode(hasher.finalize())
}
