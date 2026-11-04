//! HS256 JWTs for legacy sessions, shaped as the reference PDS's: access
//! tokens `typ: at+jwt` with `{scope, aud, sub, iat, exp}`, refresh tokens
//! `typ: refresh+jwt` with `{scope: com.atproto.refresh, aud, sub, jti, iat,
//! exp}`. Locus access tokens also carry `jti` = the session id, which is
//! checked against the database on every request.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

pub const TYP_ACCESS: &str = "at+jwt";
pub const TYP_REFRESH: &str = "refresh+jwt";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    pub scope: String,
    pub aud: String,
    pub sub: String,
    pub iat: i64,
    pub exp: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jti: Option<String>,
    /// Present on OAuth access tokens; a legacy Bearer token carrying it is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cnf: Option<serde_json::Value>,
    /// Present on service-auth tokens; a legacy token carrying it is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lxm: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Header {
    typ: String,
    alg: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum JwtError {
    Malformed,
    BadSignature,
    WrongType,
    Expired,
}

fn mac(secret: &[u8], data: &[u8]) -> Vec<u8> {
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(secret).expect("HMAC takes any key length");
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

/// Signs `claims` with `secret` as a `typ` token.
#[must_use]
pub fn sign(secret: &[u8], typ: &str, claims: &Claims) -> String {
    let header = Header {
        typ: typ.to_owned(),
        alg: "HS256".to_owned(),
    };
    let h = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).expect("header serializes"));
    let c = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).expect("claims serialize"));
    let signing_input = format!("{h}.{c}");
    let sig = URL_SAFE_NO_PAD.encode(mac(secret, signing_input.as_bytes()));
    format!("{signing_input}.{sig}")
}

/// Verifies a `typ` token and returns its claims. `now` is seconds since the epoch.
pub fn verify(secret: &[u8], typ: &str, token: &str, now: i64) -> Result<Claims, JwtError> {
    let mut parts = token.split('.');
    let (Some(h), Some(c), Some(s), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(JwtError::Malformed);
    };
    let header: Header = URL_SAFE_NO_PAD
        .decode(h)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or(JwtError::Malformed)?;
    if header.alg != "HS256" {
        return Err(JwtError::Malformed);
    }
    let sig = URL_SAFE_NO_PAD.decode(s).map_err(|_| JwtError::Malformed)?;
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(secret).expect("HMAC takes any key length");
    m.update(format!("{h}.{c}").as_bytes());
    m.verify_slice(&sig).map_err(|_| JwtError::BadSignature)?;
    if header.typ != typ {
        return Err(JwtError::WrongType);
    }
    let claims: Claims = URL_SAFE_NO_PAD
        .decode(c)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or(JwtError::Malformed)?;
    if claims.exp <= now {
        return Err(JwtError::Expired);
    }
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(exp: i64) -> Claims {
        Claims {
            scope: "com.atproto.access".into(),
            aud: "did:web:pds.example.com".into(),
            sub: "did:plc:alice".into(),
            iat: 100,
            exp,
            jti: Some("sess-1".into()),
            cnf: None,
            lxm: None,
        }
    }

    #[test]
    fn round_trip_and_refusals() {
        let t = sign(b"secret", TYP_ACCESS, &claims(1_000));
        assert_eq!(
            verify(b"secret", TYP_ACCESS, &t, 500).unwrap(),
            claims(1_000)
        );
        assert_eq!(
            verify(b"other", TYP_ACCESS, &t, 500),
            Err(JwtError::BadSignature)
        );
        assert_eq!(
            verify(b"secret", TYP_REFRESH, &t, 500),
            Err(JwtError::WrongType)
        );
        assert_eq!(
            verify(b"secret", TYP_ACCESS, &t, 1_000),
            Err(JwtError::Expired)
        );
        assert_eq!(
            verify(b"secret", TYP_ACCESS, "a.b", 0),
            Err(JwtError::Malformed)
        );
        // Tampered claims fail the signature.
        let mut parts: Vec<&str> = t.split('.').collect();
        let forged = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims(9_999)).unwrap());
        parts[1] = &forged;
        assert_eq!(
            verify(b"secret", TYP_ACCESS, &parts.join("."), 500),
            Err(JwtError::BadSignature)
        );
    }
}
