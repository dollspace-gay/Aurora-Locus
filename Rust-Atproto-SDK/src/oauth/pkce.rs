//! PKCE (Proof Key for Code Exchange) implementation (RFC 7636)
//!
//! PKCE protects against authorization code interception attacks by binding
//! the authorization request to the token request using cryptographic proof.

use rand::Rng;
use sha2::{Digest, Sha256};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

/// PKCE parameters for OAuth authorization code flow
///
/// ## RFC 7636 Compliance
///
/// - **code_verifier**: High-entropy cryptographic random string (128 chars, URL-safe)
/// - **code_challenge**: BASE64URL(SHA256(code_verifier))
/// - **code_challenge_method**: "S256" (SHA-256)
///
/// ## Security Notes
///
/// - The code_verifier MUST be kept secret and stored securely on the client
/// - The code_challenge is sent in the authorization request
/// - The code_verifier is sent in the token exchange request
/// - The server verifies: SHA256(received_verifier) == stored_challenge
#[derive(Debug, Clone)]
pub struct PkceParams {
    /// The code verifier (128 URL-safe characters)
    /// MUST be kept secret and used in token exchange
    pub code_verifier: String,

    /// The code challenge (BASE64URL(SHA256(code_verifier)))
    /// Sent in the authorization request
    pub code_challenge: String,

    /// The code challenge method (always "S256")
    pub code_challenge_method: String,
}

impl PkceParams {
    /// Generate new PKCE parameters
    ///
    /// Creates a cryptographically secure random code_verifier (128 chars)
    /// and derives the code_challenge using SHA-256.
    ///
    /// ## Example
    ///
    /// ```
    /// use atproto::oauth::PkceParams;
    ///
    /// let pkce = PkceParams::generate();
    /// assert_eq!(pkce.code_verifier.len(), 128);
    /// assert_eq!(pkce.code_challenge_method, "S256");
    /// ```
    pub fn generate() -> Self {
        let code_verifier = Self::generate_code_verifier();
        let code_challenge = Self::generate_code_challenge(&code_verifier);

        Self {
            code_verifier,
            code_challenge,
            code_challenge_method: "S256".to_string(),
        }
    }

    /// Generate a cryptographically secure code verifier
    ///
    /// Creates a 128-character URL-safe random string using characters:
    /// - A-Z, a-z, 0-9, -, _, . ~
    ///
    /// RFC 7636 requires 43-128 characters. We use 128 for maximum entropy.
    fn generate_code_verifier() -> String {
        const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
        const VERIFIER_LENGTH: usize = 128;

        let mut rng = rand::thread_rng();
        (0..VERIFIER_LENGTH)
            .map(|_| {
                let idx = rng.gen_range(0..CHARSET.len());
                CHARSET[idx] as char
            })
            .collect()
    }

    /// Generate code challenge from code verifier
    ///
    /// Computes: BASE64URL(SHA256(code_verifier))
    ///
    /// ## Arguments
    ///
    /// * `code_verifier` - The code verifier string
    ///
    /// ## Returns
    ///
    /// Base64URL-encoded SHA-256 hash of the code verifier (no padding)
    fn generate_code_challenge(code_verifier: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(code_verifier.as_bytes());
        let hash = hasher.finalize();
        URL_SAFE_NO_PAD.encode(hash)
    }

    /// Verify that a code verifier matches this challenge
    ///
    /// Used for testing and validation.
    ///
    /// ## Arguments
    ///
    /// * `verifier` - The code verifier to verify
    ///
    /// ## Returns
    ///
    /// `true` if SHA256(verifier) matches the stored challenge
    pub fn verify_verifier(&self, verifier: &str) -> bool {
        Self::generate_code_challenge(verifier) == self.code_challenge
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_code_verifier() {
        let verifier = PkceParams::generate_code_verifier();

        // Should be exactly 128 characters
        assert_eq!(verifier.len(), 128);

        // Should only contain allowed characters
        for c in verifier.chars() {
            assert!(
                c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_' || c == '~',
                "Invalid character in code verifier: {}", c
            );
        }
    }

    #[test]
    fn test_generate_code_challenge() {
        // Test vector from RFC 7636 Appendix B
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let expected_challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

        let challenge = PkceParams::generate_code_challenge(verifier);
        assert_eq!(challenge, expected_challenge);
    }

    #[test]
    fn test_generate_pkce_params() {
        let pkce = PkceParams::generate();

        // Verifier should be 128 chars
        assert_eq!(pkce.code_verifier.len(), 128);

        // Challenge should be base64url encoded (43 chars for SHA-256)
        assert_eq!(pkce.code_challenge.len(), 43);

        // Method should be S256
        assert_eq!(pkce.code_challenge_method, "S256");

        // Verify the challenge matches the verifier
        assert!(pkce.verify_verifier(&pkce.code_verifier));
    }

    #[test]
    fn test_verify_verifier() {
        let pkce = PkceParams::generate();

        // Should verify with correct verifier
        assert!(pkce.verify_verifier(&pkce.code_verifier));

        // Should not verify with wrong verifier
        assert!(!pkce.verify_verifier("wrong_verifier"));
    }

    #[test]
    fn test_generate_unique_verifiers() {
        // Generate multiple verifiers and ensure they're all different
        let verifiers: Vec<String> = (0..100)
            .map(|_| PkceParams::generate_code_verifier())
            .collect();

        // Check uniqueness (extremely unlikely to have duplicates with 128 chars)
        for i in 0..verifiers.len() {
            for j in (i + 1)..verifiers.len() {
                assert_ne!(verifiers[i], verifiers[j], "Generated duplicate verifiers");
            }
        }
    }

    #[test]
    fn test_challenge_deterministic() {
        let verifier = "test_verifier_123";

        // Same verifier should always produce same challenge
        let challenge1 = PkceParams::generate_code_challenge(verifier);
        let challenge2 = PkceParams::generate_code_challenge(verifier);

        assert_eq!(challenge1, challenge2);
    }
}
