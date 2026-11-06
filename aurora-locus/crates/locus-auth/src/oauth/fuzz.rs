//! Robustness of the OAuth input parsers: authorization (PAR) parameters
//! and their validation, client metadata, compact JWS, JWKs, DPoP proofs
//! (arbitrary strings, and properly signed proofs with arbitrary claims),
//! and the token endpoint's PKCE check. Each must answer with an error,
//! never panic.

use proptest::prelude::*;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::client::{Client, ClientMetadata};
use super::dpop::{Nonces, check_proof, tests::proof};
use super::jose::{Es256Key, Jws, PublicJwk, b64, thumbprint};
use super::request::{Parameters, check_code_grant, is_code_verifier, narrow, validate};

/// Arbitrary JSON, a few levels deep.
fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        any::<f64>()
            .prop_map(|f| serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number)),
        ".{0,12}".prop_map(Value::from),
        Just(json!("code")),
        Just(json!("S256")),
        Just(json!("atproto transition:generic")),
        Just(json!("https://app.example.com/cb")),
    ];
    leaf.prop_recursive(3, 24, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::btree_map(".{0,6}", inner, 0..4)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

const KEYS: &[&str] = &[
    "client_id",
    "response_type",
    "redirect_uri",
    "scope",
    "state",
    "code_challenge",
    "code_challenge_method",
    "dpop_jkt",
    "response_mode",
    "nonce",
    "max_age",
    "claims",
    "login_hint",
    "ui_locales",
    "id_token_hint",
    "display",
    "prompt",
    "authorization_details",
    "redirect_uris",
    "grant_types",
    "response_types",
    "token_endpoint_auth_method",
    "token_endpoint_auth_signing_alg",
    "dpop_bound_access_tokens",
    "application_type",
    "jwks",
    "jwks_uri",
    "client_uri",
    "logo_uri",
];

/// An object over the parsers' own keys (and some others).
fn object() -> impl Strategy<Value = Map<String, Value>> {
    let key = prop_oneof![
        4 => prop::sample::select(KEYS).prop_map(str::to_owned),
        1 => ".{0,8}",
    ];
    prop::collection::btree_map(key, json_value(), 0..10).prop_map(|m| m.into_iter().collect())
}

fn client() -> Client {
    let m = ClientMetadata::parse(&json!({
        "client_id": "https://app.example.com/meta.json",
        "scope": "atproto transition:generic repo:app.bsky.feed.post",
        "redirect_uris": ["https://app.example.com/cb"],
        "grant_types": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_method": "none",
        "dpop_bound_access_tokens": true,
    }))
    .unwrap();
    Client {
        id: "https://app.example.com/meta.json".into(),
        jwks: None,
        metadata: m,
        is_trusted: false,
        is_first_party: false,
    }
}

const NOW: i64 = 1_700_000_000_000;
const VERIFIER: &str = "verifier-verifier-verifier-verifier-verifier";

/// One DPoP claim: usually a valid value, sometimes absent or garbage.
fn claim(valid: Value) -> impl Strategy<Value = Option<Value>> {
    prop_oneof![
        6 => Just(Some(valid)),
        1 => Just(None),
        2 => json_value().prop_map(Some),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1024, ..ProptestConfig::default() })]

    #[test]
    fn authorization_parameters_parse_and_validate_without_panicking(
        overlay in prop::collection::btree_map(
            prop::sample::select(KEYS).prop_map(str::to_owned), json_value(), 0..3),
        approved in prop::option::of(prop_oneof![
            Just("atproto".to_owned()),
            Just("atproto transition:generic".to_owned()),
            ".{0,30}",
        ]),
        verifier in prop::option::of(prop_oneof![Just(VERIFIER.to_owned()), ".{0,60}"]),
    ) {
        // A valid request with a few fields replaced, so most cases get
        // past the first checks.
        let mut m = json!({
            "client_id": "https://app.example.com/meta.json",
            "response_type": "code",
            "redirect_uri": "https://app.example.com/cb",
            "scope": "atproto transition:generic",
            "code_challenge": b64(&Sha256::digest(VERIFIER.as_bytes())),
            "code_challenge_method": "S256",
        })
        .as_object()
        .unwrap()
        .clone();
        m.extend(overlay);
        if let Ok(p) = Parameters::parse(&m) {
            let _ = p.scopes();
            if let Ok(p) = validate(&client(), p) {
                if let Ok(p) = narrow(p, approved.as_deref()) {
                    let _ = check_code_grant(&p, "https://app.example.com/cb", verifier.as_deref());
                }
            }
        }
        if let Some(v) = &verifier {
            let _ = is_code_verifier(v);
        }
    }

    #[test]
    fn client_metadata_parses_without_panicking(m in object()) {
        let _ = ClientMetadata::parse(&Value::Object(m));
    }

    #[test]
    fn compact_jws_and_jwks_parse_without_panicking(
        s in "[A-Za-z0-9_.-]{0,80}",
        jwk in object(),
        kty in prop::sample::select(&["EC", "RSA", "OKP", "oct", ""][..]),
        crv in prop::sample::select(&["P-256", "secp256k1", "P-384", "P-521", "Ed25519", ""][..]),
        x in prop::collection::vec(any::<u8>(), 0..70),
        y in prop::collection::vec(any::<u8>(), 0..70),
    ) {
        if let Ok(jws) = Jws::decode(&s) {
            let _ = (jws.alg(), jws.typ_is("dpop+jwt"));
        }
        let mut jwk = jwk;
        jwk.insert("kty".into(), json!(kty));
        jwk.insert("crv".into(), json!(crv));
        jwk.insert("x".into(), json!(b64(&x)));
        jwk.insert("y".into(), json!(b64(&y)));
        jwk.insert("n".into(), json!(b64(&x)));
        jwk.insert("e".into(), json!(b64(&y)));
        if let Ok(k) = PublicJwk::from_jwk(&jwk) {
            for alg in ["ES256", "ES256K", "ES384", "ES512", "RS256", "PS256", "none"] {
                let _ = k.verify(alg, b"input", &x);
            }
        }
        let _ = thumbprint(&jwk);
    }

    #[test]
    fn dpop_headers_are_refused_without_panicking(
        headers in prop::collection::vec(".{0,120}", 0..3),
        method in prop::sample::select(&["GET", "POST", ""][..]),
    ) {
        let nonces = Nonces::new(&[7u8; 32]);
        let url = url::Url::parse("https://pds.example.com/xrpc/x").unwrap();
        let refs: Vec<&str> = headers.iter().map(String::as_str).collect();
        let r = check_proof(&refs, method, &url, Some("tok"), &nonces, NOW);
        prop_assert!(refs.is_empty() || r.is_err());
    }

    /// Correctly signed proofs with arbitrary claims reach every check.
    #[test]
    fn signed_dpop_proofs_with_arbitrary_claims_never_panic(
        jti in claim(json!("j")),
        htm in claim(json!("GET")),
        htu in prop_oneof![
            4 => claim(json!("https://pds.example.com/xrpc/x")),
            1 => Just(Some(json!("https://user:pw@pds.example.com/xrpc/x"))),
            1 => Just(Some(json!("ftp://pds.example.com/xrpc/x"))),
        ],
        iat in prop_oneof![
            3 => claim(json!(NOW / 1000)),
            1 => (-1_000i64..1_000).prop_map(|d| Some(json!(NOW / 1000 + d))),
        ],
        exp in prop_oneof![claim(json!(NOW / 1000 + 60)), Just(Some(json!(0)))],
        nonce in claim(json!(Nonces::new(&[7u8; 32]).next(NOW))),
        token in prop::option::of(".{0,20}"),
        ath in prop_oneof![3 => Just(None), 1 => json_value().prop_map(Some)],
        extra in object(),
    ) {
        let nonces = Nonces::new(&[7u8; 32]);
        let key = Es256Key::generate();
        let url = url::Url::parse("https://pds.example.com/xrpc/x?q=1").unwrap();
        let mut c = extra;
        for (k, v) in [("jti", jti), ("htm", htm), ("htu", htu), ("iat", iat), ("exp", exp), ("nonce", nonce)] {
            match v {
                Some(v) => c.insert(k.into(), v),
                None => c.remove(k),
            };
        }
        match (ath, &token) {
            (Some(a), _) => {
                c.insert("ath".into(), a);
            }
            (None, Some(t)) => {
                c.insert("ath".into(), json!(b64(&Sha256::digest(t.as_bytes()))));
            }
            (None, None) => {}
        }
        let p = proof(&key, Value::Object(c));
        let _ = check_proof(&[&p], "GET", &url, token.as_deref(), &nonces, NOW);
    }
}
