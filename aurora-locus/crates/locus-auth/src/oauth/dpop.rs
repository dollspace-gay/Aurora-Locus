//! DPoP (RFC 9449) as the reference's `DpopManager`: proofs are checked
//! (signature with the embedded JWK, `typ`, age, `htm`, `htu`, `ath`) and
//! must carry a server nonce: an HMAC of a 60-second counter under a secret
//! every node shares, so any node accepts the previous, current and next
//! nonce. Replay of a proof's `jti` is checked by the caller (shared store).

use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::error::OAuthError;
use super::jose::{Jws, PublicJwk, VERIFY_ALGS, b64, thumbprint};

/// How long a nonce stays valid (the reference's `DPOP_NONCE_MAX_AGE`).
pub const NONCE_MAX_AGE_MS: i64 = 3 * 60_000;
/// The nonce rotation interval (a third of the max age).
pub const ROTATION_MS: i64 = NONCE_MAX_AGE_MS / 3;
/// How long a proof's `jti` is remembered (max age + 10%).
pub const JTI_WINDOW_MS: i64 = NONCE_MAX_AGE_MS * 11 / 10;

/// Server nonces from a shared secret.
#[derive(Clone)]
pub struct Nonces {
    secret: Vec<u8>,
}

impl std::fmt::Debug for Nonces {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Nonces")
    }
}

impl Nonces {
    #[must_use]
    pub fn new(secret: &[u8]) -> Self {
        Self {
            secret: secret.to_vec(),
        }
    }

    fn compute(&self, counter: i64) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.secret).expect("any key length");
        mac.update(&counter.to_be_bytes());
        b64(&mac.finalize().into_bytes())
    }

    fn counter(now_ms: i64) -> i64 {
        now_ms.div_euclid(ROTATION_MS)
    }

    /// The nonce to hand out (the next window's, as the reference).
    #[must_use]
    pub fn next(&self, now_ms: i64) -> String {
        self.compute(Self::counter(now_ms) + 1)
    }

    /// Whether a nonce is the previous, current or next one.
    #[must_use]
    pub fn check(&self, nonce: &str, now_ms: i64) -> bool {
        let c = Self::counter(now_ms);
        [c - 1, c, c + 1].iter().any(|n| self.compute(*n) == nonce)
    }
}

/// A checked proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DpopProof {
    pub jti: String,
    /// The key's thumbprint (what tokens are bound to).
    pub jkt: String,
    pub htm: String,
    pub htu: String,
}

fn invalid(m: impl Into<String>) -> OAuthError {
    OAuthError::invalid_dpop_proof(m)
}

/// `origin + path` of a URL, as the reference's `normalizeHtuUrl` (query and
/// fragment dropped, dot segments resolved, default ports dropped).
fn normalize_htu(url: &url::Url) -> String {
    format!("{}{}", url.origin().ascii_serialization(), url.path())
}

/// Checks the request's `DPoP` header. None when there's none.
///
/// `url` is the request's full URL as the client addressed it; `access_token`
/// is set on resource requests (the proof must carry its hash in `ath`).
pub fn check_proof(
    dpop_headers: &[&str],
    method: &str,
    url: &url::Url,
    access_token: Option<&str>,
    nonces: &Nonces,
    now_ms: i64,
) -> Result<Option<DpopProof>, OAuthError> {
    let proof = match dpop_headers {
        [] => return Ok(None),
        [""] => return Err(invalid("DPoP header cannot be empty")),
        [p] => *p,
        _ => return Err(invalid("DPoP header must contain a single proof")),
    };
    let fail = |m: &str| invalid(format!("Failed to verify DPoP proof: {m}"));
    let jws = Jws::decode(proof).map_err(|e| fail(&e.0))?;
    if !jws.typ_is("dpop+jwt") {
        return Err(fail("unexpected \"typ\" JWT header value"));
    }
    let alg = jws.alg().to_owned();
    if !VERIFY_ALGS.contains(&alg.as_str()) {
        return Err(fail(
            "\"alg\" (Algorithm) Header Parameter value not allowed",
        ));
    }
    let jwk = jws
        .header
        .get("jwk")
        .and_then(Value::as_object)
        .ok_or_else(|| fail("\"jwk\" (JSON Web Key) Header Parameter must be a JSON object"))?
        .clone();
    let key = PublicJwk::from_jwk(&jwk).map_err(|e| fail(&e.0))?;
    key.verify(&alg, &jws.signing_input, &jws.signature)
        .map_err(|e| fail(&e.0))?;

    // iat present, at most 10 s old, with the nonce lifetime as tolerance.
    let now = now_ms / 1000;
    let tolerance = NONCE_MAX_AGE_MS / 1000;
    let iat = jws
        .payload
        .get("iat")
        .and_then(Value::as_i64)
        .ok_or_else(|| fail("\"iat\" claim must be a number"))?;
    if now - iat - tolerance > 10 {
        return Err(fail(
            "\"iat\" claim timestamp check failed (too far in the past)",
        ));
    }
    if iat - now > tolerance {
        return Err(fail(
            "\"iat\" claim timestamp check failed (it should be in the past)",
        ));
    }
    if let Some(exp) = jws.payload.get("exp") {
        let exp = exp
            .as_i64()
            .ok_or_else(|| fail("\"exp\" claim must be a number"))?;
        if exp <= now - tolerance {
            return Err(fail("\"exp\" claim timestamp check failed"));
        }
    }

    let p = &jws.payload;
    let nonce = match p.get("nonce") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(invalid("Invalid DPoP \"nonce\" type")),
    };
    let jti = p
        .get("jti")
        .and_then(Value::as_str)
        .filter(|j| !j.is_empty())
        .ok_or_else(|| invalid("DPoP \"jti\" missing"))?
        .to_owned();
    let htm = p.get("htm").and_then(Value::as_str).unwrap_or("");
    if htm.is_empty() || htm != method {
        return Err(invalid("DPoP \"htm\" mismatch"));
    }
    let htu = p
        .get("htu")
        .and_then(Value::as_str)
        .filter(|h| !h.is_empty())
        .ok_or_else(|| invalid("Invalid DPoP \"htu\" type"))?
        .to_owned();
    let parsed = url::Url::parse(&htu).map_err(|_| invalid("DPoP \"htu\" is not a valid URL"))?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(invalid("DPoP \"htu\" must not contain credentials"));
    }
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(invalid("DPoP \"htu\" must be http or https"));
    }
    if normalize_htu(&parsed) != normalize_htu(url) {
        return Err(invalid("DPoP \"htu\" mismatch"));
    }
    match &nonce {
        None => return Err(OAuthError::use_dpop_nonce(None)),
        Some(n) if !nonces.check(n, now_ms) => {
            return Err(OAuthError::use_dpop_nonce(Some("DPoP \"nonce\" mismatch")));
        }
        Some(_) => {}
    }
    match (access_token, p.get("ath")) {
        (Some(token), ath) => {
            let want = b64(&Sha256::digest(token.as_bytes()));
            if ath.and_then(Value::as_str) != Some(want.as_str()) {
                return Err(invalid("DPoP \"ath\" mismatch"));
            }
        }
        (None, Some(_)) => return Err(invalid("DPoP \"ath\" claim not allowed")),
        (None, None) => {}
    }
    let jkt = thumbprint(&jwk).map_err(|_| invalid("Failed to calculate jkt"))?;
    Ok(Some(DpopProof {
        jti,
        jkt,
        htm: htm.to_owned(),
        htu,
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::oauth::jose::Es256Key;
    use serde_json::{Map, json};

    /// A proof signed by `key` (its public JWK embedded).
    pub(crate) fn proof(key: &Es256Key, claims: Value) -> String {
        let mut header = Map::new();
        header.insert("typ".into(), json!("dpop+jwt"));
        let mut jwk = key.jwk();
        for k in ["kid", "use", "alg"] {
            jwk.as_object_mut().unwrap().remove(k);
        }
        header.insert("jwk".into(), jwk);
        key.sign(header, &claims)
    }

    #[test]
    fn proofs_are_checked_as_the_reference() {
        let nonces = Nonces::new(&[7u8; 32]);
        let now = 1_700_000_000_000i64;
        let key = Es256Key::generate();
        let url = url::Url::parse("https://pds.example.com/xrpc/app.bsky.feed.getTimeline?limit=5")
            .unwrap();
        let good = json!({
            "jti": "j1", "htm": "GET", "htu": "https://pds.example.com/xrpc/app.bsky.feed.getTimeline",
            "iat": now / 1000, "nonce": nonces.next(now), "ath": b64(&Sha256::digest(b"tok")),
        });
        let p = proof(&key, good.clone());
        let ok = check_proof(&[&p], "GET", &url, Some("tok"), &nonces, now)
            .unwrap()
            .unwrap();
        assert_eq!(ok.jti, "j1");
        assert_eq!(ok.jkt, thumbprint(key.jwk().as_object().unwrap()).unwrap());
        assert!(
            check_proof(&[], "GET", &url, None, &nonces, now)
                .unwrap()
                .is_none()
        );

        let with = |k: &str, v: Value| {
            let mut c = good.clone();
            if v.is_null() {
                c.as_object_mut().unwrap().remove(k);
            } else {
                c[k] = v;
            }
            proof(&key, c)
        };
        let err_of = |p: &str, token: Option<&str>| {
            check_proof(&[p], "GET", &url, token, &nonces, now).unwrap_err()
        };
        assert_eq!(
            err_of(&with("htm", json!("POST")), Some("tok")).description,
            "DPoP \"htm\" mismatch"
        );
        assert_eq!(
            err_of(
                &with("htu", json!("https://other.example/xrpc/x")),
                Some("tok")
            )
            .description,
            "DPoP \"htu\" mismatch"
        );
        let e = err_of(&with("nonce", Value::Null), Some("tok"));
        assert_eq!((e.error.as_str(), e.status), ("use_dpop_nonce", 400));
        assert_eq!(
            err_of(&with("nonce", json!("stale")), Some("tok")).description,
            "DPoP \"nonce\" mismatch"
        );
        assert_eq!(
            err_of(&p, Some("other")).description,
            "DPoP \"ath\" mismatch"
        );
        assert_eq!(
            err_of(&p, None).description,
            "DPoP \"ath\" claim not allowed"
        );
        assert_eq!(
            err_of(&with("jti", Value::Null), Some("tok")).description,
            "DPoP \"jti\" missing"
        );
        assert!(
            err_of(&with("iat", json!(now / 1000 - 600)), Some("tok"))
                .description
                .contains("too far in the past")
        );
        // A nonce from the previous window still works; two windows back doesn't.
        let earlier = nonces.next(now - ROTATION_MS);
        assert!(nonces.check(&earlier, now));
        assert!(!nonces.check(&nonces.next(now - 3 * ROTATION_MS), now));
        // A signature by another key fails.
        let other = Es256Key::generate();
        let mut forged = proof(&other, good.clone());
        let tail = forged.rsplit_once('.').unwrap().1.to_owned();
        let head = p.rsplit_once('.').unwrap().0;
        forged = format!("{head}.{tail}");
        assert!(
            err_of(&forged, Some("tok"))
                .description
                .starts_with("Failed to verify DPoP proof")
        );
    }
}
