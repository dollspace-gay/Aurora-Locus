//! Inter-service JWTs, as the reference's `createServiceJwt` and
//! `verifyJwt`: signed with an account's (or service's) atproto signing
//! key, `{iat, iss, aud, exp, lxm?, jti}`, ES256K or ES256.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use proto_blue_crypto::Keypair;
use serde_json::{Value, json};
use thiserror::Error;

/// Default lifetime when the caller names no `exp`, as the reference.
pub const DEFAULT_TTL_S: i64 = 60;

/// The longest inbound lifetime, and the clock skew allowed on top.
pub const MAX_LIFETIME_S: i64 = 3600;
pub const SKEW_S: i64 = 60;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ServiceJwtError {
    #[error("poorly formatted jwt")]
    BadJwt,
    #[error("Invalid jwt type")]
    BadJwtType,
    #[error("jwt expired")]
    JwtExpired,
    #[error("jwt audience does not match service did")]
    BadJwtAudience,
    #[error("bad or missing jwt lexicon method (\"lxm\"); must match: {0}")]
    BadJwtLexiconMethod(String),
    #[error("jwt iss is not a valid did")]
    BadJwtIss,
    #[error("jwt signature does not match jwt issuer")]
    BadJwtSignature,
    #[error("jwt lifetime is longer than an hour")]
    BadJwtLifetime,
    #[error("jwt has no jti")]
    NoJti,
    #[error("jwt replayed")]
    Replayed,
    #[error("signing failed: {0}")]
    Sign(String),
}

impl ServiceJwtError {
    /// The atproto error name the reference returns.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::BadJwt | Self::Sign(_) | Self::BadJwtLifetime | Self::NoJti | Self::Replayed => {
                "BadJwt"
            }
            Self::BadJwtType => "BadJwtType",
            Self::JwtExpired => "JwtExpired",
            Self::BadJwtAudience => "BadJwtAudience",
            Self::BadJwtLexiconMethod(_) => "BadJwtLexiconMethod",
            Self::BadJwtIss => "BadJwtIss",
            Self::BadJwtSignature => "BadJwtSignature",
        }
    }
}

/// Makes a service JWT. `exp` is seconds since the epoch; the default is a
/// minute from `now`.
pub fn create(
    key: &dyn Keypair,
    iss: &str,
    aud: &str,
    exp: Option<i64>,
    lxm: Option<&str>,
    now: i64,
) -> Result<String, ServiceJwtError> {
    let header = json!({"typ": "JWT", "alg": key.jwt_alg()});
    let mut payload = json!({
        "iat": now,
        "iss": iss,
        "aud": aud,
        "exp": exp.unwrap_or(now + DEFAULT_TTL_S),
        "jti": hex::encode(rand::random::<[u8; 16]>()),
    });
    if let Some(lxm) = lxm {
        payload["lxm"] = json!(lxm);
    }
    let signing_input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(payload.to_string())
    );
    let sig = key
        .sign(signing_input.as_bytes())
        .map_err(|e| ServiceJwtError::Sign(e.to_string()))?;
    Ok(format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(sig)))
}

/// A service JWT whose claims passed the checks that need no key.
#[derive(Debug, Clone)]
pub struct Unverified {
    pub iss: String,
    pub aud: String,
    pub exp: i64,
    pub iat: Option<i64>,
    pub lxm: Option<String>,
    pub jti: Option<String>,
    signing_input: String,
    sig: Vec<u8>,
}

impl Unverified {
    /// The DID part of `iss` and the verification method to check with:
    /// `atproto_label` for `#atproto_labeler`, otherwise `atproto`.
    #[must_use]
    pub fn issuer_key_ref(&self) -> (&str, &'static str) {
        match self.iss.split_once('#') {
            Some((did, "atproto_labeler")) => (did, "atproto_label"),
            Some((did, _)) => (did, "atproto"),
            None => (&self.iss, "atproto"),
        }
    }

    /// The inbound lifetime cap: `exp - iat` at most an hour plus a
    /// minute's skew, or without `iat`, `exp - now` at most an hour.
    #[must_use]
    pub fn lifetime_ok(&self, now: i64) -> bool {
        match self.iat {
            Some(iat) => self.exp - iat <= MAX_LIFETIME_S + SKEW_S,
            None => self.exp - now <= MAX_LIFETIME_S,
        }
    }

    /// Whether the signature verifies against `did_key`.
    #[must_use]
    pub fn verifies_with(&self, did_key: &str) -> bool {
        signature_ok(did_key, self.signing_input.as_bytes(), &self.sig)
    }
}

/// An atproto signature check: compact low-S signatures only.
fn signature_ok(did_key: &str, input: &[u8], sig: &[u8]) -> bool {
    proto_blue_crypto::verify_signature(did_key, input, sig, false).unwrap_or(false)
}

/// Parses a service JWT and checks what needs no key, in the reference's
/// order: not an access, refresh or DPoP token; not expired; for `lxm`,
/// naming that method; a DID (or DID#service) issuer.
pub fn parse(token: &str, lxm: Option<&str>, now: i64) -> Result<Unverified, ServiceJwtError> {
    let parts: Vec<&str> = token.split('.').collect();
    let [h, p, s] = parts[..] else {
        return Err(ServiceJwtError::BadJwt);
    };
    let decode = |part: &str| -> Result<Value, ServiceJwtError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(part)
            .map_err(|_| ServiceJwtError::BadJwt)?;
        serde_json::from_slice(&bytes).map_err(|_| ServiceJwtError::BadJwt)
    };
    let header = decode(h)?;
    if matches!(
        header["typ"].as_str(),
        Some("at+jwt" | "refresh+jwt" | "dpop+jwt" | "kryphocron-audience+jwt")
    ) {
        return Err(ServiceJwtError::BadJwtType);
    }
    let payload = decode(p)?;
    let exp = payload["exp"].as_i64().ok_or(ServiceJwtError::BadJwt)?;
    if now > exp {
        return Err(ServiceJwtError::JwtExpired);
    }
    let claimed = payload["lxm"].as_str().map(str::to_owned);
    if let Some(want) = lxm
        && claimed.as_deref() != Some(want)
    {
        return Err(ServiceJwtError::BadJwtLexiconMethod(want.to_owned()));
    }
    let iss = payload["iss"]
        .as_str()
        .ok_or(ServiceJwtError::BadJwtIss)?
        .to_owned();
    let did = iss.split('#').next().unwrap_or("");
    if proto_blue_syntax::Did::new(did).is_err() {
        return Err(ServiceJwtError::BadJwtIss);
    }
    Ok(Unverified {
        iss,
        aud: payload["aud"].as_str().unwrap_or_default().to_owned(),
        exp,
        iat: payload["iat"].as_i64(),
        lxm: claimed,
        jti: payload["jti"].as_str().map(str::to_owned),
        signing_input: format!("{h}.{p}"),
        sig: URL_SAFE_NO_PAD
            .decode(s)
            .map_err(|_| ServiceJwtError::BadJwt)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto_blue_crypto::K256Keypair;

    /// The interop signature fixtures (P-256 and K-256, high-S and DER
    /// forms included) give the fixture's verdict.
    #[test]
    fn interop_signature_fixtures() {
        let path = format!(
            "{}/../../interop-test-files/crypto/signature-fixtures.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let cases: Vec<Value> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(cases.len() >= 6);
        let b64 = base64::engine::general_purpose::STANDARD_NO_PAD;
        for c in &cases {
            let msg = b64.decode(c["messageBase64"].as_str().unwrap()).unwrap();
            let sig = b64.decode(c["signatureBase64"].as_str().unwrap()).unwrap();
            let key = c["publicKeyDid"].as_str().unwrap();
            assert_eq!(
                signature_ok(key, &msg, &sig),
                c["validSignature"].as_bool().unwrap(),
                "{}",
                c["comment"]
            );
        }
    }

    #[test]
    fn create_and_check() {
        let key = K256Keypair::generate();
        let t = create(
            &key,
            "did:plc:abc",
            "did:web:pds.test",
            None,
            Some("com.atproto.server.createAccount"),
            1000,
        )
        .unwrap();
        let u = parse(&t, Some("com.atproto.server.createAccount"), 1000).unwrap();
        assert_eq!(
            (u.iss.as_str(), u.aud.as_str(), u.exp),
            ("did:plc:abc", "did:web:pds.test", 1060)
        );
        assert!(u.verifies_with(&key.did()));
        assert!(!u.verifies_with(&K256Keypair::generate().did()));
        assert_eq!(
            parse(&t, Some("com.atproto.repo.createRecord"), 1000)
                .unwrap_err()
                .name(),
            "BadJwtLexiconMethod"
        );
        assert_eq!(
            parse(&t, None, 1061).unwrap_err(),
            ServiceJwtError::JwtExpired
        );
        assert_eq!(parse("a.b", None, 0).unwrap_err(), ServiceJwtError::BadJwt);
        let labeler = create(
            &key,
            "did:plc:abc#atproto_labeler",
            "did:web:x",
            None,
            None,
            0,
        )
        .unwrap();
        assert_eq!(
            parse(&labeler, None, 0).unwrap().issuer_key_ref(),
            ("did:plc:abc", "atproto_label")
        );
        // A tampered payload no longer verifies.
        let mut parts: Vec<String> = t.split('.').map(str::to_owned).collect();
        parts[1] =
            URL_SAFE_NO_PAD.encode(r#"{"iss":"did:plc:evil","aud":"did:web:pds.test","exp":9999}"#);
        assert!(
            !parse(&parts.join("."), None, 0)
                .unwrap()
                .verifies_with(&key.did())
        );
    }
}
