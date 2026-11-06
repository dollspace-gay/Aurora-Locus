//! The JOSE pieces OAuth needs: compact JWS decoding, public JWKs for every
//! algorithm the reference verifies (RS*, PS*, ES256, ES256K, ES384,
//! ES512), RFC 7638 thumbprints, and ES256 signing for access tokens.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256, Sha384, Sha512};

/// The algorithms the reference verifies (`VERIFY_ALGOS`).
pub const VERIFY_ALGS: &[&str] = &[
    "RS256", "RS384", "RS512", "PS256", "PS384", "PS512", "ES256", "ES256K", "ES384", "ES512",
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct JoseError(pub String);

fn err(m: impl Into<String>) -> JoseError {
    JoseError(m.into())
}

#[must_use]
pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn unb64(s: &str) -> Result<Vec<u8>, JoseError> {
    URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|_| err("Invalid base64url encoding"))
}

/// A decoded compact JWS (signature not yet checked).
#[derive(Debug, Clone)]
pub struct Jws {
    pub header: Map<String, Value>,
    pub payload: Map<String, Value>,
    pub signing_input: Vec<u8>,
    pub signature: Vec<u8>,
}

impl Jws {
    pub fn decode(compact: &str) -> Result<Self, JoseError> {
        let mut parts = compact.split('.');
        let (Some(h), Some(p), Some(s), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(err("Invalid Compact JWS"));
        };
        let header: Value =
            serde_json::from_slice(&unb64(h)?).map_err(|_| err("Invalid JWS Protected Header"))?;
        let payload: Value = serde_json::from_slice(&unb64(p)?)
            .map_err(|_| err("JWT Claims Set must be a top-level JSON object"))?;
        let (Value::Object(header), Value::Object(payload)) = (header, payload) else {
            return Err(err("JWT Claims Set must be a top-level JSON object"));
        };
        Ok(Self {
            header,
            payload,
            signing_input: format!("{h}.{p}").into_bytes(),
            signature: unb64(s)?,
        })
    }

    #[must_use]
    pub fn alg(&self) -> &str {
        self.header.get("alg").and_then(Value::as_str).unwrap_or("")
    }

    /// The `typ` header compared as jose does (case-insensitive, an
    /// `application/` prefix ignored).
    #[must_use]
    pub fn typ_is(&self, want: &str) -> bool {
        let norm = |t: &str| {
            let t = t.to_ascii_lowercase();
            t.strip_prefix("application/")
                .map(str::to_owned)
                .unwrap_or(t)
        };
        self.header
            .get("typ")
            .and_then(Value::as_str)
            .is_some_and(|t| norm(t) == norm(want))
    }
}

/// A public key from a JWK.
#[derive(Clone)]
pub enum PublicJwk {
    P256(p256::ecdsa::VerifyingKey),
    K256(k256::ecdsa::VerifyingKey),
    P384(p384::ecdsa::VerifyingKey),
    P521(Box<p521::ecdsa::VerifyingKey>),
    Rsa(rsa::RsaPublicKey),
}

impl std::fmt::Debug for PublicJwk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::P256(_) => "PublicJwk(P-256)",
            Self::K256(_) => "PublicJwk(secp256k1)",
            Self::P384(_) => "PublicJwk(P-384)",
            Self::P521(_) => "PublicJwk(P-521)",
            Self::Rsa(_) => "PublicJwk(RSA)",
        })
    }
}

fn coord(jwk: &Map<String, Value>, k: &str, len: usize) -> Result<Vec<u8>, JoseError> {
    let v = unb64(
        jwk.get(k)
            .and_then(Value::as_str)
            .ok_or_else(|| err(format!("JWK \"{k}\" missing")))?,
    )?;
    if v.len() != len {
        return Err(err(format!("Invalid JWK \"{k}\"")));
    }
    Ok(v)
}

impl PublicJwk {
    /// A public JWK (one holding a private part is refused, as jose does
    /// for embedded keys).
    pub fn from_jwk(jwk: &Map<String, Value>) -> Result<Self, JoseError> {
        if jwk.contains_key("d") {
            return Err(err(
                "\"jwk\" (JSON Web Key) Header Parameter must be a public key",
            ));
        }
        let kty = jwk.get("kty").and_then(Value::as_str).unwrap_or("");
        match kty {
            "EC" => {
                let crv = jwk.get("crv").and_then(Value::as_str).unwrap_or("");
                let bad = || err("Invalid EC key");
                match crv {
                    "P-256" => {
                        let (x, y) = (coord(jwk, "x", 32)?, coord(jwk, "y", 32)?);
                        let pt = p256::EncodedPoint::from_affine_coordinates(
                            x.as_slice().into(),
                            y.as_slice().into(),
                            false,
                        );
                        Ok(Self::P256(
                            p256::ecdsa::VerifyingKey::from_encoded_point(&pt)
                                .map_err(|_| bad())?,
                        ))
                    }
                    "secp256k1" => {
                        let (x, y) = (coord(jwk, "x", 32)?, coord(jwk, "y", 32)?);
                        let pt = k256::EncodedPoint::from_affine_coordinates(
                            x.as_slice().into(),
                            y.as_slice().into(),
                            false,
                        );
                        Ok(Self::K256(
                            k256::ecdsa::VerifyingKey::from_encoded_point(&pt)
                                .map_err(|_| bad())?,
                        ))
                    }
                    "P-384" => {
                        let (x, y) = (coord(jwk, "x", 48)?, coord(jwk, "y", 48)?);
                        let pt = p384::EncodedPoint::from_affine_coordinates(
                            x.as_slice().into(),
                            y.as_slice().into(),
                            false,
                        );
                        Ok(Self::P384(
                            p384::ecdsa::VerifyingKey::from_encoded_point(&pt)
                                .map_err(|_| bad())?,
                        ))
                    }
                    "P-521" => {
                        let (x, y) = (coord(jwk, "x", 66)?, coord(jwk, "y", 66)?);
                        let pt = p521::EncodedPoint::from_affine_coordinates(
                            x.as_slice().into(),
                            y.as_slice().into(),
                            false,
                        );
                        Ok(Self::P521(Box::new(
                            p521::ecdsa::VerifyingKey::from_encoded_point(&pt)
                                .map_err(|_| bad())?,
                        )))
                    }
                    _ => Err(err(format!("Unsupported curve: {crv}"))),
                }
            }
            "RSA" => {
                let n = unb64(
                    jwk.get("n")
                        .and_then(Value::as_str)
                        .ok_or_else(|| err("JWK \"n\" missing"))?,
                )?;
                let e = unb64(
                    jwk.get("e")
                        .and_then(Value::as_str)
                        .ok_or_else(|| err("JWK \"e\" missing"))?,
                )?;
                let key = rsa::RsaPublicKey::new(
                    rsa::BigUint::from_bytes_be(&n),
                    rsa::BigUint::from_bytes_be(&e),
                )
                .map_err(|_| err("Invalid RSA key"))?;
                if rsa::traits::PublicKeyParts::size(&key) * 8 < 2048 {
                    return Err(err("RSA keys must be at least 2048 bits"));
                }
                Ok(Self::Rsa(key))
            }
            other => Err(err(format!("Unsupported key type: {other}"))),
        }
    }

    /// Checks `sig` over `input` with `alg`; the algorithm must fit the key.
    pub fn verify(&self, alg: &str, input: &[u8], sig: &[u8]) -> Result<(), JoseError> {
        use rsa::signature::Verifier;
        let failed = || err("signature verification failed");
        let mismatch = || err(format!("\"alg\" ({alg}) doesn't fit the key"));
        match (self, alg) {
            (Self::P256(k), "ES256") => {
                let s = p256::ecdsa::Signature::from_slice(sig).map_err(|_| failed())?;
                k.verify(input, &s).map_err(|_| failed())
            }
            (Self::K256(k), "ES256K") => {
                let s = k256::ecdsa::Signature::from_slice(sig).map_err(|_| failed())?;
                // Accept high-S signatures, as jose does.
                let s = s.normalize_s().unwrap_or(s);
                k.verify(input, &s).map_err(|_| failed())
            }
            (Self::P384(k), "ES384") => {
                let s = p384::ecdsa::Signature::from_slice(sig).map_err(|_| failed())?;
                k.verify(input, &s).map_err(|_| failed())
            }
            (Self::P521(k), "ES512") => {
                let s = p521::ecdsa::Signature::from_slice(sig).map_err(|_| failed())?;
                k.verify(input, &s).map_err(|_| failed())
            }
            (Self::Rsa(k), alg) if alg.starts_with("RS") || alg.starts_with("PS") => {
                let s = rsa::pkcs1v15::Signature::try_from(sig).map_err(|_| failed())?;
                let p = rsa::pss::Signature::try_from(sig).map_err(|_| failed())?;
                let ok = match alg {
                    "RS256" => rsa::pkcs1v15::VerifyingKey::<Sha256>::new(k.clone())
                        .verify(input, &s)
                        .is_ok(),
                    "RS384" => rsa::pkcs1v15::VerifyingKey::<Sha384>::new(k.clone())
                        .verify(input, &s)
                        .is_ok(),
                    "RS512" => rsa::pkcs1v15::VerifyingKey::<Sha512>::new(k.clone())
                        .verify(input, &s)
                        .is_ok(),
                    "PS256" => rsa::pss::VerifyingKey::<Sha256>::new(k.clone())
                        .verify(input, &p)
                        .is_ok(),
                    "PS384" => rsa::pss::VerifyingKey::<Sha384>::new(k.clone())
                        .verify(input, &p)
                        .is_ok(),
                    "PS512" => rsa::pss::VerifyingKey::<Sha512>::new(k.clone())
                        .verify(input, &p)
                        .is_ok(),
                    _ => return Err(mismatch()),
                };
                if ok { Ok(()) } else { Err(failed()) }
            }
            _ => Err(mismatch()),
        }
    }
}

/// The RFC 7638 SHA-256 thumbprint of a public JWK (base64url).
pub fn thumbprint(jwk: &Map<String, Value>) -> Result<String, JoseError> {
    let get = |k: &str| {
        jwk.get(k)
            .and_then(Value::as_str)
            .ok_or_else(|| err(format!("JWK \"{k}\" missing")))
    };
    let canonical = match jwk.get("kty").and_then(Value::as_str) {
        Some("EC") => format!(
            r#"{{"crv":{},"kty":"EC","x":{},"y":{}}}"#,
            Value::from(get("crv")?),
            Value::from(get("x")?),
            Value::from(get("y")?)
        ),
        Some("RSA") => format!(
            r#"{{"e":{},"kty":"RSA","n":{}}}"#,
            Value::from(get("e")?),
            Value::from(get("n")?)
        ),
        Some("OKP") => format!(
            r#"{{"crv":{},"kty":"OKP","x":{}}}"#,
            Value::from(get("crv")?),
            Value::from(get("x")?)
        ),
        _ => return Err(err("Unsupported key type")),
    };
    Ok(b64(&Sha256::digest(canonical.as_bytes())))
}

// ---------------------------------------------------------------- signing

/// The ES256 key OAuth access tokens are signed with **[deviation]** (the
/// reference signs with the legacy HS256 secret).
#[derive(Clone)]
pub struct Es256Key {
    key: p256::ecdsa::SigningKey,
    kid: String,
}

impl std::fmt::Debug for Es256Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Es256Key")
            .field("kid", &self.kid)
            .finish_non_exhaustive()
    }
}

impl Es256Key {
    #[must_use]
    pub fn generate() -> Self {
        Self::from_signing(p256::ecdsa::SigningKey::random(&mut rand::rngs::OsRng))
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, JoseError> {
        let key =
            p256::ecdsa::SigningKey::from_slice(bytes).map_err(|_| err("invalid P-256 key"))?;
        Ok(Self::from_signing(key))
    }

    fn from_signing(key: p256::ecdsa::SigningKey) -> Self {
        let mut me = Self {
            key,
            kid: String::new(),
        };
        let public = me.public_jwk();
        me.kid = public
            .as_object()
            .and_then(|m| thumbprint(m).ok())
            .unwrap_or_default();
        me
    }

    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        self.key.to_bytes().to_vec()
    }

    #[must_use]
    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// The public JWK (without `kid`, `use` and `alg`).
    fn public_jwk(&self) -> Value {
        let pt = self.key.verifying_key().to_encoded_point(false);
        json!({
            "kty": "EC",
            "crv": "P-256",
            "x": b64(pt.x().map(|x| x.as_slice()).unwrap_or_default()),
            "y": b64(pt.y().map(|y| y.as_slice()).unwrap_or_default()),
        })
    }

    /// The key as published in `/oauth/jwks`.
    #[must_use]
    pub fn jwk(&self) -> Value {
        let mut v = self.public_jwk();
        v["kid"] = json!(self.kid);
        v["use"] = json!("sig");
        v["alg"] = json!("ES256");
        v
    }

    /// A compact JWS with this key (`alg` ES256, `kid` set).
    #[must_use]
    pub fn sign(&self, mut header: Map<String, Value>, payload: &Value) -> String {
        use p256::ecdsa::signature::Signer;
        header.insert("alg".into(), json!("ES256"));
        header.insert("kid".into(), json!(self.kid));
        let input = format!(
            "{}.{}",
            b64(Value::Object(header).to_string().as_bytes()),
            b64(payload.to_string().as_bytes())
        );
        let sig: p256::ecdsa::Signature = self.key.sign(input.as_bytes());
        format!("{input}.{}", b64(&sig.to_bytes()))
    }

    /// Verifies a JWS this key signed.
    pub fn verify(&self, jws: &Jws) -> Result<(), JoseError> {
        if jws.alg() != "ES256" {
            return Err(err("unexpected \"alg\""));
        }
        PublicJwk::P256(*self.key.verifying_key()).verify(
            "ES256",
            &jws.signing_input,
            &jws.signature,
        )
    }
}

/// Verifies an HS256 JWS under `secret` (OAuth access tokens a reference
/// PDS issued before an import: it signs them with its JWT secret).
pub fn verify_hs256(secret: &[u8], jws: &Jws) -> Result<(), JoseError> {
    use hmac::{Hmac, Mac};
    if jws.alg() != "HS256" {
        return Err(err("unexpected \"alg\""));
    }
    let mut mac =
        <Hmac<sha2::Sha256> as Mac>::new_from_slice(secret).map_err(|_| err("invalid HMAC key"))?;
    mac.update(&jws.signing_input);
    mac.verify_slice(&jws.signature)
        .map_err(|_| err("signature verification failed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hs256_verifies_only_under_its_secret() {
        use hmac::{Hmac, Mac};
        let input = format!(
            "{}.{}",
            b64(br#"{"alg":"HS256","typ":"at+jwt"}"#),
            b64(br#"{"sub":"did:plc:a"}"#)
        );
        let mut mac = <Hmac<sha2::Sha256> as Mac>::new_from_slice(b"secret").unwrap();
        mac.update(input.as_bytes());
        let jwt = format!("{input}.{}", b64(&mac.finalize().into_bytes()));
        let jws = Jws::decode(&jwt).unwrap();
        verify_hs256(b"secret", &jws).unwrap();
        assert!(verify_hs256(b"other", &jws).is_err());
        let k = Es256Key::generate();
        let es = Jws::decode(&k.sign(Map::new(), &json!({"a": 1}))).unwrap();
        assert!(verify_hs256(b"secret", &es).is_err());
    }

    #[test]
    fn es256_signs_verifies_and_thumbprints() {
        let k = Es256Key::generate();
        let jwt = k.sign(Map::new(), &json!({"a": 1}));
        let jws = Jws::decode(&jwt).unwrap();
        k.verify(&jws).unwrap();
        let public = PublicJwk::from_jwk(k.jwk().as_object().unwrap()).unwrap();
        public
            .verify("ES256", &jws.signing_input, &jws.signature)
            .unwrap();
        assert!(public.verify("ES256", b"other", &jws.signature).is_err());
        assert!(
            public
                .verify("RS256", &jws.signing_input, &jws.signature)
                .is_err()
        );
        // Round trip through bytes keeps the same kid.
        assert_eq!(Es256Key::from_bytes(&k.to_bytes()).unwrap().kid(), k.kid());
    }

    #[test]
    fn rfc7638_example_thumbprint() {
        // RFC 7638 §3.1.
        let jwk = json!({
            "kty": "RSA",
            "n": "0vx7agoebGcQSuuPiLJXZptN9nndrQmbXEps2aiAFbWhM78LhWx4cbbfAAtVT86zwu1RK7aPFFxuhDR1L6tSoc_BJECPebWKRXjBZCiFV4n3oknjhMstn64tZ_2W-5JsGY4Hc5n9yBXArwl93lqt7_RN5w6Cf0h4QyQ5v-65YGjQR0_FDW2QvzqY368QQMicAtaSqzs8KJZgnYb9c7d0zgdAZHzu6qMQvRL5hajrn1n91CbOpbISD08qNLyrdkt-bFTWhAI4vMQFh6WeZu0fM4lFd2NcRwr3XPksINHaQ-G_xBniIqbw0Ls1jF44-csFCur-kEgU8awapJzKnqDKgw",
            "e": "AQAB",
            "alg": "RS256",
            "kid": "2011-04-29"
        });
        assert_eq!(
            thumbprint(jwk.as_object().unwrap()).unwrap(),
            "NzbLsXh8uDCcd-6MNwXF4W_7noWXFZAfHkxZsRGC9Xs"
        );
    }

    #[test]
    fn other_algorithms_verify() {
        use rsa::signature::{RandomizedSigner, SignatureEncoding, Signer};
        let input = b"header.payload";
        // ES256K, including a high-S signature.
        let k = k256::ecdsa::SigningKey::random(&mut rand::rngs::OsRng);
        let pt = k.verifying_key().to_encoded_point(false);
        let jwk = json!({"kty": "EC", "crv": "secp256k1", "x": b64(pt.x().unwrap()), "y": b64(pt.y().unwrap())});
        let sig: k256::ecdsa::Signature = k.sign(input);
        let public = PublicJwk::from_jwk(jwk.as_object().unwrap()).unwrap();
        public.verify("ES256K", input, &sig.to_bytes()).unwrap();
        let (r, s_) = sig.split_scalars();
        let high = k256::ecdsa::Signature::from_scalars(r, -*s_).unwrap();
        public.verify("ES256K", input, &high.to_bytes()).unwrap();
        // ES384.
        let k = p384::ecdsa::SigningKey::random(&mut rand::rngs::OsRng);
        let pt = k.verifying_key().to_encoded_point(false);
        let jwk = json!({"kty": "EC", "crv": "P-384", "x": b64(pt.x().unwrap()), "y": b64(pt.y().unwrap())});
        let sig: p384::ecdsa::Signature = k.sign(input);
        PublicJwk::from_jwk(jwk.as_object().unwrap())
            .unwrap()
            .verify("ES384", input, &sig.to_bytes())
            .unwrap();
        // RS256 and PS256.
        let private = rsa::RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048).unwrap();
        let public_key = private.to_public_key();
        use rsa::traits::PublicKeyParts;
        let jwk = json!({"kty": "RSA", "n": b64(&public_key.n().to_bytes_be()), "e": b64(&public_key.e().to_bytes_be())});
        let public = PublicJwk::from_jwk(jwk.as_object().unwrap()).unwrap();
        let rs = rsa::pkcs1v15::SigningKey::<Sha256>::new(private.clone()).sign(input);
        public.verify("RS256", input, &rs.to_vec()).unwrap();
        assert!(public.verify("RS384", input, &rs.to_vec()).is_err());
        let ps = rsa::pss::SigningKey::<Sha256>::new(private)
            .sign_with_rng(&mut rand::rngs::OsRng, input);
        public.verify("PS256", input, &ps.to_vec()).unwrap();
        assert!(public.verify("ES256", input, &ps.to_vec()).is_err());
    }

    #[test]
    fn private_keys_are_refused() {
        let mut jwk = Es256Key::generate().jwk();
        jwk["d"] = json!("AAAA");
        assert!(PublicJwk::from_jwk(jwk.as_object().unwrap()).is_err());
    }
}
