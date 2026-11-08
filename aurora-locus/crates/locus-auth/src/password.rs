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

/// The marker of a hash imported from a reference PDS: `scrypt:salt:hexkey`.
pub const SCRYPT_MARKER: &str = "scrypt:";

/// The reference's scrypt (Node `crypto.scrypt` defaults: N = 16384, r = 8,
/// p = 1, a 64-byte key; the salt is the hex string's bytes), as hex.
fn reference_scrypt(password: &str, salt: &str) -> Option<String> {
    let params = scrypt::Params::new(14, 8, 1, 64).ok()?;
    let mut out = [0u8; 64];
    scrypt::scrypt(password.as_bytes(), salt.as_bytes(), &params, &mut out).ok()?;
    Some(hex::encode(out))
}

/// Whether a stored hash is an imported scrypt one (rehash it on the next
/// successful login).
#[must_use]
pub fn is_imported(stored: &str) -> bool {
    stored.starts_with(SCRYPT_MARKER)
}

/// The reference's app-password hash for an account, `salt:hexkey` (salt =
/// hex of the first 16 bytes of sha256(did)), with the marker: imported app
/// passwords are found by equality with it.
#[must_use]
pub fn reference_app_password_hash(did: &str, password: &str) -> Option<String> {
    use sha2::Digest;
    let salt = hex::encode(&Sha256::digest(did.as_bytes())[..16]);
    reference_scrypt(password, &salt).map(|k| format!("{SCRYPT_MARKER}{salt}:{k}"))
}

/// Whether `password` matches a stored hash: Argon2id, or an imported
/// scrypt hash. Malformed hashes never match.
#[must_use]
pub fn verify(password: &str, stored: &str) -> bool {
    if password.len() > MAX_LOGIN_PASSWORD {
        return false;
    }
    if let Some(rest) = stored.strip_prefix(SCRYPT_MARKER) {
        let Some((salt, key)) = rest.split_once(':') else {
            return false;
        };
        return reference_scrypt(password, salt)
            .is_some_and(|k| subtle::ConstantTimeEq::ct_eq(k.as_bytes(), key.as_bytes()).into());
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
    fn imported_scrypt_hashes() {
        // scrypt("hunter2-hunter2", "0123456789abcdef", N=16384, r=8, p=1,
        // 64 bytes), as the reference's hashWithSalt makes it.
        let stored = format!("{SCRYPT_MARKER}0123456789abcdef:{REFERENCE_KEY}");
        assert!(is_imported(&stored));
        assert!(verify("hunter2-hunter2", &stored));
        assert!(!verify("hunter2-hunter3", &stored));
        assert!(!verify("hunter2-hunter2", "scrypt:no-separator"));
        assert!(!is_imported(&hash("x").unwrap()));
        let ap = reference_app_password_hash("did:plc:a", "abcd-efgh-ijkl-mnop").unwrap();
        assert!(verify("abcd-efgh-ijkl-mnop", &ap));
        assert_ne!(
            ap,
            reference_app_password_hash("did:plc:b", "abcd-efgh-ijkl-mnop").unwrap()
        );
    }

    const REFERENCE_KEY: &str = "49521831b3670ff7fb057bb83f23c981bf62cbfff86340ac3a9d234504b3c764\
                                 913ff5309bbbb143125958ff649a9a38091228128ed2bbd21f67b247f23e5a01";

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
