//! Admin TOTP 2FA (chainlink #442): RFC-6238 code verification + secret
//! encryption-at-rest.
//!
//! The secret is stored encrypted (AES-256-GCM) in
//! `admin_security_config.totp_secret_encrypted`, keyed by
//! `PDS_ADMIN_TOTP_ENCRYPTION_KEY_HEX`. When no key is configured the cipher is
//! absent and enrollment refuses — a TOTP secret is never written in plaintext.

use crate::error::{PdsError, PdsResult};
use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::Aes256Gcm;
use base64::Engine as _;
use totp_rs::{Algorithm, Secret, TOTP};

/// Issuer label shown in authenticator apps.
const TOTP_ISSUER: &str = "Aurora-Locus Admin";
/// RFC-6238 parameters: SHA1, 6 digits, 30s step, ±1 step (±30s) skew tolerance.
const TOTP_DIGITS: usize = 6;
const TOTP_SKEW: u8 = 1;
const TOTP_STEP: u64 = 30;
/// AES-GCM nonce length (96-bit, the standard).
const NONCE_LEN: usize = 12;

/// AES-256-GCM cipher for admin TOTP secrets at rest.
#[derive(Clone)]
pub struct AdminTotpCipher {
    key: [u8; 32],
}

impl AdminTotpCipher {
    /// Build from a 64-char (32-byte) hex key.
    pub fn from_hex(hex_key: &str) -> PdsResult<Self> {
        let bytes = hex::decode(hex_key.trim()).map_err(|_| {
            PdsError::Validation("PDS_ADMIN_TOTP_ENCRYPTION_KEY_HEX is not valid hex".into())
        })?;
        let key: [u8; 32] = bytes.try_into().map_err(|_| {
            PdsError::Validation(
                "PDS_ADMIN_TOTP_ENCRYPTION_KEY_HEX must be 32 bytes (64 hex chars)".into(),
            )
        })?;
        Ok(Self { key })
    }

    /// Build from optional config: `None` when no key is configured, in which
    /// case TOTP enrollment refuses rather than persisting a plaintext secret.
    pub fn from_config(hex_key: Option<&str>) -> PdsResult<Option<Self>> {
        match hex_key {
            Some(h) if !h.trim().is_empty() => Ok(Some(Self::from_hex(h)?)),
            _ => Ok(None),
        }
    }

    fn cipher(&self) -> Aes256Gcm {
        // The key is a compile-time-fixed 32 bytes, so an AES-256 init cannot
        // fail on length.
        Aes256Gcm::new_from_slice(&self.key).expect("AES-256 key is exactly 32 bytes")
    }

    /// Encrypt a secret → `base64(nonce || ciphertext)`. A fresh random nonce
    /// per call.
    pub fn encrypt(&self, plaintext: &[u8]) -> PdsResult<String> {
        // `generate_nonce` yields a random 96-bit nonce; using it by inference
        // avoids naming the (deprecated) GenericArray type in our code.
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ct = self
            .cipher()
            .encrypt(&nonce, plaintext)
            .map_err(|_| PdsError::Internal("TOTP secret encryption failed".into()))?;
        let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
        // Index via the [u8] Deref rather than the deprecated `as_slice`.
        out.extend_from_slice(&nonce[..]);
        out.extend_from_slice(&ct);
        Ok(base64::engine::general_purpose::STANDARD.encode(out))
    }

    /// Decrypt a `base64(nonce || ciphertext)` blob back to the raw secret.
    pub fn decrypt(&self, stored: &str) -> PdsResult<Vec<u8>> {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(stored.trim())
            .map_err(|_| PdsError::Internal("stored TOTP secret is not valid base64".into()))?;
        if raw.len() <= NONCE_LEN {
            return Err(PdsError::Internal("stored TOTP secret is truncated".into()));
        }
        let (nonce_bytes, ct) = raw.split_at(NONCE_LEN);
        // Rebuild the nonce by overwriting a fresh instance's bytes — again, no
        // explicit GenericArray reference. `nonce_bytes` is exactly NONCE_LEN.
        let mut nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        nonce.copy_from_slice(nonce_bytes);
        self.cipher()
            .decrypt(&nonce, ct)
            .map_err(|_| PdsError::Internal("TOTP secret decryption failed (wrong key?)".into()))
    }
}

/// A freshly generated enrollment secret + its user-facing forms.
pub struct TotpEnrollment {
    /// Raw secret bytes — encrypt before persisting.
    pub secret_bytes: Vec<u8>,
    /// Base32 secret, for manual entry into an authenticator app.
    pub secret_base32: String,
    /// `otpauth://` provisioning URI, for QR / one-tap import.
    pub provisioning_uri: String,
}

/// Generate a new TOTP enrollment labelled for `account_label` (the admin's DID
/// or handle). RFC-6238 SHA1/6-digit/30s, the near-universal authenticator
/// default.
pub fn generate_enrollment(account_label: &str) -> PdsResult<TotpEnrollment> {
    let secret_bytes = Secret::generate_secret()
        .to_bytes()
        .map_err(|e| PdsError::Internal(format!("TOTP secret generation failed: {e}")))?;
    let secret_base32 =
        base32::encode(base32::Alphabet::Rfc4648 { padding: false }, &secret_bytes);
    let provisioning_uri = format!(
        "otpauth://totp/{issuer}:{label}?secret={secret}&issuer={issuer}&algorithm=SHA1&digits={digits}&period={step}",
        issuer = urlencoding::encode(TOTP_ISSUER),
        label = urlencoding::encode(account_label),
        secret = secret_base32,
        digits = TOTP_DIGITS,
        step = TOTP_STEP,
    );
    Ok(TotpEnrollment {
        secret_bytes,
        secret_base32,
        provisioning_uri,
    })
}

/// Verify a submitted code against the raw secret bytes, within the skew window.
pub fn verify_code(secret_bytes: Vec<u8>, code: &str) -> PdsResult<bool> {
    let totp = TOTP::new(Algorithm::SHA1, TOTP_DIGITS, TOTP_SKEW, TOTP_STEP, secret_bytes)
        .map_err(|e| PdsError::Internal(format!("invalid TOTP configuration: {e}")))?;
    totp.check_current(code)
        .map_err(|e| PdsError::Internal(format!("system clock error verifying TOTP: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key_hex() -> String {
        "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff".to_string()
    }

    #[test]
    fn cipher_round_trips() {
        let cipher = AdminTotpCipher::from_hex(&test_key_hex()).unwrap();
        let secret = b"a-20-byte-totp-secret";
        let stored = cipher.encrypt(secret).unwrap();
        // Nonce is random → ciphertext differs each call.
        assert_ne!(stored, cipher.encrypt(secret).unwrap());
        assert_eq!(cipher.decrypt(&stored).unwrap(), secret);
    }

    #[test]
    fn decrypt_with_wrong_key_fails() {
        let a = AdminTotpCipher::from_hex(&test_key_hex()).unwrap();
        let b = AdminTotpCipher::from_hex(
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        )
        .unwrap();
        let stored = a.encrypt(b"secret").unwrap();
        assert!(b.decrypt(&stored).is_err());
    }

    #[test]
    fn from_config_absent_key_is_none() {
        assert!(AdminTotpCipher::from_config(None).unwrap().is_none());
        assert!(AdminTotpCipher::from_config(Some("  ")).unwrap().is_none());
        assert!(AdminTotpCipher::from_config(Some(&test_key_hex()))
            .unwrap()
            .is_some());
    }

    #[test]
    fn from_hex_rejects_bad_length_and_non_hex() {
        assert!(AdminTotpCipher::from_hex("abcd").is_err());
        assert!(AdminTotpCipher::from_hex("not-hex!!").is_err());
    }

    #[test]
    fn generate_then_verify_round_trips() {
        let enrollment = generate_enrollment("did:plc:testadmin").unwrap();
        assert!(enrollment.provisioning_uri.starts_with("otpauth://totp/"));
        assert!(!enrollment.secret_base32.is_empty());

        // Compute the current code from the same secret, then verify it.
        let totp = TOTP::new(
            Algorithm::SHA1,
            TOTP_DIGITS,
            TOTP_SKEW,
            TOTP_STEP,
            enrollment.secret_bytes.clone(),
        )
        .unwrap();
        let code = totp.generate_current().unwrap();
        assert!(verify_code(enrollment.secret_bytes.clone(), &code).unwrap());
        // A wrong code is rejected (pick one that differs from the live code so
        // this can't collide with the current window).
        let wrong = if code == "000000" { "111111" } else { "000000" };
        assert!(!verify_code(enrollment.secret_bytes, wrong).unwrap());
    }
}
