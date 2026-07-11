//! Holder-mediated signing channel (v0.10 Arc 2 Phase δ / LOCKED §5).
//!
//! v0.10 status (chainlink #448): did:web accounts hold their `#atproto` signing
//! key ON THE PDS, identical to did:plc — so getServiceAuth / entryway-auth /
//! repo commits sign in-process, and this channel is the FALLBACK consulted only
//! when an account has no PDS-held key. In v0.10 no such account is reachable
//! (there is no did:web creation route), so the default channel below returns an
//! honest "not yet wired" 4xx if it is ever hit.
//!
//! v0.11 (Phase γ / did:web sovereignty, chainlink #447): a *sovereign* did:web
//! account publishes a public key the substrate does not hold. Any operation
//! that must sign *as the holder* then cannot sign in-process; it must reach out
//! over this channel to wherever the holder's `#atproto` key lives — replacing
//! the placeholder below with a real channel consuming the (pending) async
//! signer trait.
//!
//! [`HolderSigningChannel`] is that seam. Phase δ ships the seam, its two
//! JWT-signing dispatch sites (getServiceAuth + entryway-auth), and
//! [`UnavailableHolderSigningChannel`] — the default implementation, which
//! returns an honest "not yet wired" error. Phase γ ships the real channel
//! infrastructure (SD-A4 (a): the same channel that carries commit-signing,
//! with a discriminated payload) and installs it in place of the placeholder.
//!
//! Phase δ's trait has exactly one method — `sign_service_auth`, the sole
//! consumer at this phase (getServiceAuth + entryway). Phase γ **extends** the
//! trait with `sign_commit` when it lands the commit-signing consumer; a
//! commit-signing method declared now, with no caller, would trip the lib/bin
//! dead-code tax (the same reason `HolderSigningError` carries only its one
//! constructed variant).

use axum::async_trait;

use crate::error::PdsError;

/// Delegates cryptographic signing to the account holder rather than the
/// substrate — the v0.11 sovereignty path, where the substrate does not hold the
/// `#atproto` private key. In v0.10 did:web accounts hold their key on the PDS
/// (parity with did:plc), so this seam is the fallback for keyless accounts;
/// the real channel lands in v0.11 (Phase γ, chainlink #447 / #448).
///
/// [`sign_service_auth`](HolderSigningChannel::sign_service_auth) returns the
/// **64-byte compact `R‖S` ES256K signature** — the shape `RepoSigner::sign`
/// and browser WebCrypto both produce, so a holder client signs uniformly
/// regardless of message class. The getServiceAuth / entryway JWT assemblers
/// re-encode that to DER for the JWT wire format
/// (`service_auth::verify_service_jwt` decodes with `Signature::from_der`).
/// Phase γ adds `sign_commit` (compact, consumed directly by the commit path).
#[async_trait]
pub trait HolderSigningChannel: Send + Sync {
    /// Sign a JWT signing input (`base64url(header) + "." + base64url(payload)`)
    /// for a holder-issued service-auth / entryway-auth token. Under SD-A4 (a)
    /// this rides the same channel as commit-signing (Phase γ) with a
    /// discriminated payload; the holder client auto-approves (the token is
    /// short-lived and narrow-audience) rather than prompting.
    async fn sign_service_auth(
        &self,
        did: &str,
        signing_input: &[u8],
    ) -> Result<Vec<u8>, HolderSigningError>;
}

/// Failure modes of a holder-signing dispatch.
///
/// Phase δ constructs only [`HolderSigningError::ChannelNotAvailable`] — the
/// placeholder's sole outcome. Phase γ extends this enum with the real dispatch
/// failure modes (holder unreachable, rejected-by-holder, timed-out) as it
/// implements them; the `From<HolderSigningError> for PdsError` match below
/// forces each new variant to declare its client-facing mapping at that point.
/// (No speculative variants are declared here — an unconstructed variant would
/// trip the lib/bin dead-code tax.)
#[derive(Debug, thiserror::Error)]
pub enum HolderSigningError {
    /// The holder-signing channel has not been wired yet (Phase γ pending).
    #[error("holder signing channel not yet available (Phase γ pending)")]
    ChannelNotAvailable,
}

impl From<HolderSigningError> for PdsError {
    fn from(e: HolderSigningError) -> Self {
        let msg = e.to_string();
        match e {
            // 4xx-class: a did:web holder asking the substrate to mediate a
            // signature before the channel exists is a client-visible
            // capability state, not a server fault. Mirrors the pre-δ
            // rejection's status (`PdsError::Validation` → 400) so did:web
            // getServiceAuth stays a 400 — no user-visible regression, just an
            // honester message.
            HolderSigningError::ChannelNotAvailable => PdsError::Validation(msg),
        }
    }
}

/// The default [`HolderSigningChannel`]: every call returns
/// [`HolderSigningError::ChannelNotAvailable`]. Installed at `AppContext`
/// construction until Phase γ swaps in the real channel.
///
/// This is not a stub-for-elision — it is a real implementation whose contract
/// is "this channel is not yet wired," which the dispatch sites handle as a
/// clean 4xx rather than a panicking placeholder.
pub struct UnavailableHolderSigningChannel;

#[async_trait]
impl HolderSigningChannel for UnavailableHolderSigningChannel {
    async fn sign_service_auth(
        &self,
        _did: &str,
        _signing_input: &[u8],
    ) -> Result<Vec<u8>, HolderSigningError> {
        Err(HolderSigningError::ChannelNotAvailable)
    }
}

/// Test-only [`HolderSigningChannel`] that signs with a fixed in-memory
/// secp256k1 key, returning the 64-byte compact ES256K signature the real
/// holder-side contract specifies. Shared by the getServiceAuth + entryway
/// dispatch-site tests so they can drive the "channel returns a real
/// signature" path that the production `UnavailableHolderSigningChannel`
/// cannot (that path goes live for real only when Phase γ lands).
#[cfg(test)]
pub struct MockHolderSigningChannel {
    signing_key: k256::ecdsa::SigningKey,
}

#[cfg(test)]
impl Default for MockHolderSigningChannel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl MockHolderSigningChannel {
    pub fn new() -> Self {
        Self {
            signing_key: k256::ecdsa::SigningKey::random(&mut rand::rngs::OsRng),
        }
    }

    /// The public key the mock's signatures verify against (for tests that
    /// assemble a JWT and verify it end-to-end).
    pub fn verifying_key(&self) -> k256::ecdsa::VerifyingKey {
        *self.signing_key.verifying_key()
    }

    fn sign_compact(&self, msg: &[u8]) -> Vec<u8> {
        use k256::ecdsa::signature::Signer;
        let sig: k256::ecdsa::Signature = self.signing_key.sign(msg);
        sig.to_bytes().to_vec() // 64-byte compact R‖S
    }
}

#[cfg(test)]
#[async_trait]
impl HolderSigningChannel for MockHolderSigningChannel {
    async fn sign_service_auth(
        &self,
        _did: &str,
        signing_input: &[u8],
    ) -> Result<Vec<u8>, HolderSigningError> {
        Ok(self.sign_compact(signing_input))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unavailable_channel_rejects_service_auth() {
        let ch = UnavailableHolderSigningChannel;
        assert!(matches!(
            ch.sign_service_auth("did:web:x.example.com", b"h.p").await,
            Err(HolderSigningError::ChannelNotAvailable)
        ));
    }

    #[test]
    fn channel_not_available_maps_to_4xx_validation() {
        let err: PdsError = HolderSigningError::ChannelNotAvailable.into();
        // Validation is the 400-class variant (matches the pre-δ rejection).
        assert!(matches!(err, PdsError::Validation(_)));
        assert!(err.to_string().contains("Phase γ pending"));
    }
}
