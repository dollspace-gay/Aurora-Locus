//! Account passwords and app passwords.
//!
//! Passwords are hashed with Argon2id. App passwords look like the
//! reference's (`xxxx-xxxx-xxxx-xxxx`, base32 letters and digits); each also
//! has a lookup hash (HMAC-SHA256 under a server pepper) so a login finds the
//! one candidate to verify instead of trying every app password.

use argon2::Argon2;
use argon2::password_hash::{
    PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Longest password accepted at login (the reference accepts 512).
pub const MAX_LOGIN_PASSWORD: usize = 512;
/// Longest new password (as the reference).
pub const MAX_NEW_PASSWORD: usize = 256;

pub fn hash(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

/// Whether `password` matches a stored Argon2id hash. Malformed hashes never match.
#[must_use]
pub fn verify(password: &str, stored: &str) -> bool {
    if password.len() > MAX_LOGIN_PASSWORD {
        return false;
    }
    let Ok(parsed) = PasswordHash::new(stored) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// A new app password, `xxxx-xxxx-xxxx-xxxx`.
#[must_use]
pub fn new_app_password() -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let bytes: [u8; 16] = rand::random();
    let chars: Vec<char> = bytes
        .iter()
        .map(|b| ALPHABET[(*b % 32) as usize] as char)
        .collect();
    chars
        .chunks(4)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-")
}

/// Whether a string has the app-password shape.
#[must_use]
pub fn looks_like_app_password(s: &str) -> bool {
    s.len() == 19
        && s.split('-').count() == 4
        && s.split('-').all(|g| {
            g.len() == 4
                && g.bytes()
                    .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
        })
}

/// The lookup hash of an app password for an account.
#[must_use]
pub fn app_password_lookup(pepper: &[u8], did: &str, app_password: &str) -> String {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(pepper).expect("HMAC takes any key length");
    mac.update(did.as_bytes());
    mac.update(b"\0");
    mac.update(app_password.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify() {
        let h = hash("correct horse").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify("correct horse", &h));
        assert!(!verify("wrong horse", &h));
        assert!(!verify("correct horse", "not a hash"));
        assert!(!verify(&"x".repeat(MAX_LOGIN_PASSWORD + 1), &h));
    }

    #[test]
    fn app_passwords() {
        let p = new_app_password();
        assert!(looks_like_app_password(&p), "{p}");
        assert_ne!(p, new_app_password());
        assert!(!looks_like_app_password("not-an-app-password"));
        let a = app_password_lookup(b"pepper", "did:plc:a", &p);
        assert_eq!(a, app_password_lookup(b"pepper", "did:plc:a", &p));
        assert_ne!(a, app_password_lookup(b"pepper", "did:plc:b", &p));
        assert_ne!(a, app_password_lookup(b"other", "did:plc:a", &p));
    }
}
