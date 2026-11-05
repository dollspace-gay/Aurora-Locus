//! Time-based sign-in codes (TOTP, RFC 6238): six digits from HMAC-SHA1
//! over 30-second steps, as authenticator apps expect. One step either way
//! is accepted, for clocks a little apart; the caller makes each step
//! usable once.

use hmac::{Hmac, Mac};
use sha1::Sha1;

/// Seconds per step.
pub const STEP_S: i64 = 30;
const DIGITS: u32 = 6;

/// A new secret (160 bits, as RFC 4226 recommends).
#[must_use]
pub fn new_secret() -> Vec<u8> {
    rand::random::<[u8; 20]>().to_vec()
}

/// The secret as authenticator apps take it typed in: base32, no padding.
#[must_use]
pub fn secret_text(secret: &[u8]) -> String {
    data_encoding::BASE32_NOPAD.encode(secret)
}

/// The secret grouped in fours, for reading aloud or typing.
#[must_use]
pub fn secret_grouped(secret: &[u8]) -> String {
    secret_text(secret)
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The `otpauth://` URI apps read from a QR code or a link.
#[must_use]
pub fn uri(secret: &[u8], issuer: &str, account: &str) -> String {
    let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits={DIGITS}&period={STEP_S}",
        enc(issuer),
        enc(account),
        secret_text(secret),
        enc(issuer)
    )
}

/// The code for a step.
#[must_use]
pub fn code_at(secret: &[u8], step: i64) -> String {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("any key length");
    mac.update(&step.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let offset = usize::from(h[19] & 0x0f);
    let bin = (u32::from(h[offset]) & 0x7f) << 24
        | u32::from(h[offset + 1]) << 16
        | u32::from(h[offset + 2]) << 8
        | u32::from(h[offset + 3]);
    format!(
        "{:0width$}",
        bin % 10u32.pow(DIGITS),
        width = DIGITS as usize
    )
}

/// The step a code matches at `now_s` (± one step), if any. Spaces in the
/// code are ignored.
#[must_use]
pub fn verify(secret: &[u8], code: &str, now_s: i64) -> Option<i64> {
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let now = now_s.div_euclid(STEP_S);
    [now - 1, now, now + 1].into_iter().find(|step| {
        use subtle::ConstantTimeEq;
        bool::from(code_at(secret, *step).as_bytes().ct_eq(code.as_bytes()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238's SHA-1 test vectors (its 8-digit codes, last six digits).
    #[test]
    fn rfc_6238_vectors() {
        let secret = b"12345678901234567890";
        for (t, code) in [
            (59, "287082"),
            (1_111_111_109, "081804"),
            (1_111_111_111, "050471"),
            (1_234_567_890, "005924"),
            (2_000_000_000, "279037"),
        ] {
            assert_eq!(code_at(secret, t / STEP_S), code, "t={t}");
        }
    }

    #[test]
    fn a_code_verifies_one_step_either_way_only() {
        let secret = new_secret();
        let now = 1_700_000_000;
        let code = code_at(&secret, now / STEP_S);
        assert_eq!(verify(&secret, &code, now), Some(now / STEP_S));
        assert_eq!(verify(&secret, &code, now + STEP_S), Some(now / STEP_S));
        assert_eq!(verify(&secret, &code, now + 3 * STEP_S), None);
        let spaced = format!("{} {}", &code[..3], &code[3..]);
        assert!(verify(&secret, &spaced, now).is_some());
        assert_eq!(verify(&secret, "12345", now), None);
        assert_eq!(verify(&secret, "abcdef", now), None);
    }

    #[test]
    fn the_uri_and_text_forms() {
        let secret = b"12345678901234567890";
        assert_eq!(secret_text(secret), "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
        assert_eq!(secret_grouped(secret).split(' ').count(), 8);
        let u = uri(secret, "Example PDS", "alice");
        assert!(
            u.starts_with("otpauth://totp/Example+PDS:alice?secret=GEZDGNBV"),
            "{u}"
        );
        assert!(u.contains("&issuer=Example+PDS&"));
    }
}
