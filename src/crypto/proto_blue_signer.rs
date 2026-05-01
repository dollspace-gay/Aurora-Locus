//! Adapter that exposes our local secp256k1 signing keys as a
//! `proto_blue::crypto::Signer`.
//!
//! `proto_blue::repo::Repo` accepts a `&dyn proto_blue::crypto::Signer`
//! everywhere it commits. Our existing `PlcSigner` and
//! `Secp256k1KeyPair` types are independent of proto-blue, so we wrap
//! them in `RepoSigner` to satisfy the trait without coupling those
//! modules to proto-blue's traits directly.
//!
//! The signature path matches proto-blue's `K256Keypair`:
//! 1. SHA-256 the message
//! 2. ECDSA-sign the digest with `sign_prehash`
//! 3. Normalize to low-S form
//! 4. Return raw 64-byte (R||S) compact signature
//!
//! Step 3 (low-S normalisation) is the only behavioural delta from the
//! existing `PlcSigner::sign` — it's required for the resulting commits
//! to verify against proto-blue's `K256Keypair::verify`.

use k256::ecdsa::{signature::hazmat::PrehashSigner, Signature, SigningKey};
use proto_blue::crypto::{CryptoError, Signer};
use sha2::{Digest, Sha256};

/// `proto_blue::crypto::Signer` adapter over a borrowed `k256::ecdsa::SigningKey`.
///
/// Holds the key by value (cloned) so the adapter is `'static` and can
/// be moved across threads / into long-running closures. `SigningKey`
/// is `Clone` and small (32-byte secret), so this is cheap.
pub struct RepoSigner {
    signing_key: SigningKey,
}

impl RepoSigner {
    /// Wrap a `SigningKey` for use as a proto-blue commit signer.
    pub fn new(signing_key: SigningKey) -> Self {
        Self { signing_key }
    }

    /// Convenience: build a signer from raw 32-byte secp256k1 private key bytes.
    ///
    /// Errors if the bytes don't represent a valid scalar.
    pub fn from_bytes(private_key: &[u8]) -> Result<Self, CryptoError> {
        let signing_key = SigningKey::from_slice(private_key)
            .map_err(|e| CryptoError::VerificationFailed(format!("invalid private key: {}", e)))?;
        Ok(Self::new(signing_key))
    }
}

impl Signer for RepoSigner {
    fn jwt_alg(&self) -> &str {
        "ES256K"
    }

    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let digest = Sha256::digest(msg);
        let sig: Signature = self
            .signing_key
            .sign_prehash(&digest)
            .map_err(|e| CryptoError::VerificationFailed(format!("signing failed: {}", e)))?;
        let normalized = sig.normalize_s().unwrap_or(sig);
        Ok(normalized.to_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto_blue::crypto::{ExportableKeypair, K256Keypair, Keypair, Verifier};

    #[test]
    fn sign_verifies_under_proto_blue_k256() {
        // Round-trip: sign with our adapter, verify with proto-blue's own
        // K256Keypair. This is the property that matters — anything we sign
        // must verify in the proto-blue commit-verification path.
        let proto_kp = K256Keypair::generate();
        let private = proto_kp.export_private_key();
        let public_compressed = proto_kp.public_key_compressed();

        let adapter = RepoSigner::from_bytes(&private).unwrap();
        let msg = b"hello atproto";
        let sig = Signer::sign(&adapter, msg).unwrap();

        // proto-blue exposes verification via a separate `K256Verifier`
        // built from the compressed public key. Round-trip the signature
        // through it — the adapter's low-S normalisation guarantees the
        // verifier accepts.
        let verifier = K256Keypair::verifier_from_compressed(&public_compressed).unwrap();
        assert!(verifier.verify(msg, &sig).unwrap());
    }

    #[test]
    fn rejects_invalid_private_key() {
        // 31 bytes is too short for secp256k1; should error rather than panic.
        let bad = [0u8; 31];
        assert!(RepoSigner::from_bytes(&bad).is_err());
    }

    #[test]
    fn jwt_alg_is_es256k() {
        let kp = K256Keypair::generate();
        let adapter = RepoSigner::from_bytes(&kp.export_private_key()).unwrap();
        assert_eq!(adapter.jwt_alg(), "ES256K");
    }
}
