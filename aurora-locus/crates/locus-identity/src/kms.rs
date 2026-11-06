//! AWS KMS (or a compatible service): wrapping the master key, and signing
//! with a secp256k1 PLC rotation key that never leaves KMS, as the
//! reference's `PDS_PLC_ROTATION_KEY_KMS_KEY_ID`.
//!
//! Where KMS is and how to sign in come from the standard variables:
//! `AWS_REGION` (or `AWS_DEFAULT_REGION`), `AWS_ACCESS_KEY_ID`,
//! `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, and `LOCUS_KMS_ENDPOINT`
//! for a non-AWS endpoint.

use std::collections::HashMap;
use std::time::Duration;

use aws_sdk_kms::Client;
use aws_sdk_kms::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_kms::error::DisplayErrorContext;
use aws_sdk_kms::primitives::Blob;
use aws_sdk_kms::types::{DataKeySpec, MessageType, SigningAlgorithmSpec};
use aws_smithy_http_client::tls;
use k256::pkcs8::DecodePublicKey;
use thiserror::Error;
use zeroize::Zeroizing;

#[derive(Debug, Error)]
pub enum KmsError {
    #[error("KMS isn't configured: {0}")]
    Config(String),
    #[error("KMS: {0}")]
    Call(String),
    #[error("KMS answered something unexpected: {0}")]
    Answer(String),
}

/// A KMS client.
#[derive(Clone)]
pub struct Kms {
    client: Client,
}

impl std::fmt::Debug for Kms {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kms").finish_non_exhaustive()
    }
}

fn call<E: std::error::Error + 'static, R: std::fmt::Debug>(
    e: aws_sdk_kms::error::SdkError<E, R>,
) -> KmsError {
    KmsError::Call(DisplayErrorContext(e).to_string())
}

impl Kms {
    /// A client from the standard variables in `env`.
    pub fn from_env(env: &HashMap<String, String>) -> Result<Self, KmsError> {
        let get = |k: &str| {
            env.get(k)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let region = get("AWS_REGION")
            .or_else(|| get("AWS_DEFAULT_REGION"))
            .ok_or_else(|| KmsError::Config("set AWS_REGION".into()))?;
        let (id, secret) = match (get("AWS_ACCESS_KEY_ID"), get("AWS_SECRET_ACCESS_KEY")) {
            (Some(i), Some(s)) => (i, s),
            _ => {
                return Err(KmsError::Config(
                    "set AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY".into(),
                ));
            }
        };
        let http = aws_smithy_http_client::Builder::new()
            .tls_provider(tls::Provider::Rustls(
                tls::rustls_provider::CryptoMode::Ring,
            ))
            .build_https();
        let mut b = aws_sdk_kms::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .http_client(http)
            .region(Region::new(region))
            .credentials_provider(Credentials::new(
                id,
                secret,
                get("AWS_SESSION_TOKEN"),
                None,
                "locus",
            ))
            .timeout_config(
                aws_sdk_kms::config::timeout::TimeoutConfig::builder()
                    .connect_timeout(Duration::from_secs(10))
                    .operation_timeout(Duration::from_secs(30))
                    .build(),
            );
        if let Some(e) = get("LOCUS_KMS_ENDPOINT") {
            b = b.endpoint_url(e);
        }
        Ok(Self {
            client: Client::from_conf(b.build()),
        })
    }

    /// A new 32-byte data key under `key_id`: (the key, its wrapped form).
    pub async fn generate_data_key(
        &self,
        key_id: &str,
    ) -> Result<(Zeroizing<[u8; 32]>, Vec<u8>), KmsError> {
        let out = self
            .client
            .generate_data_key()
            .key_id(key_id)
            .key_spec(DataKeySpec::Aes256)
            .send()
            .await
            .map_err(call)?;
        let plain = out
            .plaintext()
            .ok_or_else(|| KmsError::Answer("no plaintext data key".into()))?;
        let wrapped = out
            .ciphertext_blob()
            .ok_or_else(|| KmsError::Answer("no wrapped data key".into()))?;
        let key: [u8; 32] = plain
            .as_ref()
            .try_into()
            .map_err(|_| KmsError::Answer("the data key isn't 32 bytes".into()))?;
        Ok((Zeroizing::new(key), wrapped.as_ref().to_vec()))
    }

    /// Unwraps a data key made by `generate_data_key`.
    pub async fn decrypt(
        &self,
        key_id: &str,
        wrapped: &[u8],
    ) -> Result<Zeroizing<[u8; 32]>, KmsError> {
        let out = self
            .client
            .decrypt()
            .key_id(key_id)
            .ciphertext_blob(Blob::new(wrapped.to_vec()))
            .send()
            .await
            .map_err(call)?;
        let plain = out
            .plaintext()
            .ok_or_else(|| KmsError::Answer("no plaintext".into()))?;
        let key: [u8; 32] = plain
            .as_ref()
            .try_into()
            .map_err(|_| KmsError::Answer("the data key isn't 32 bytes".into()))?;
        Ok(Zeroizing::new(key))
    }

    /// The did:key of a secp256k1 signing key in KMS.
    pub async fn k256_did_key(&self, key_id: &str) -> Result<String, KmsError> {
        let out = self
            .client
            .get_public_key()
            .key_id(key_id)
            .send()
            .await
            .map_err(call)?;
        let der = out
            .public_key()
            .ok_or_else(|| KmsError::Answer("no public key".into()))?;
        let key = k256::PublicKey::from_public_key_der(der.as_ref()).map_err(|_| {
            KmsError::Answer(format!("{key_id} isn't a secp256k1 key (ECC_SECG_P256K1)"))
        })?;
        let compressed = k256::elliptic_curve::sec1::ToEncodedPoint::to_encoded_point(&key, true);
        Ok(proto_blue_crypto::format_did_key(
            "ES256K",
            compressed.as_bytes(),
        ))
    }

    /// Signs `msg` (SHA-256 hashed by KMS) with a secp256k1 key: the
    /// 64-byte compact signature, low-S, as atproto requires.
    pub async fn sign_k256(&self, key_id: &str, msg: &[u8]) -> Result<Vec<u8>, KmsError> {
        let out = self
            .client
            .sign()
            .key_id(key_id)
            .message(Blob::new(msg.to_vec()))
            .message_type(MessageType::Raw)
            .signing_algorithm(SigningAlgorithmSpec::EcdsaSha256)
            .send()
            .await
            .map_err(call)?;
        let der = out
            .signature()
            .ok_or_else(|| KmsError::Answer("no signature".into()))?;
        let sig = k256::ecdsa::Signature::from_der(der.as_ref())
            .map_err(|_| KmsError::Answer("a malformed signature".into()))?;
        let sig = sig.normalize_s().unwrap_or(sig);
        Ok(sig.to_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_kms::types::{KeySpec, KeyUsageType};

    /// A client for `LOCUS_TEST_KMS_ENDPOINT` (local-kms in a container,
    /// say), or None to skip.
    pub(crate) fn test_kms() -> Option<Kms> {
        let endpoint = std::env::var("LOCUS_TEST_KMS_ENDPOINT")
            .ok()
            .filter(|v| !v.is_empty())?;
        let env: HashMap<String, String> = [
            ("AWS_REGION", "eu-west-2"),
            ("AWS_ACCESS_KEY_ID", "test"),
            ("AWS_SECRET_ACCESS_KEY", "test"),
            ("LOCUS_KMS_ENDPOINT", endpoint.as_str()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        Some(Kms::from_env(&env).unwrap())
    }

    async fn new_key(kms: &Kms, spec: KeySpec, usage: KeyUsageType) -> String {
        kms.client
            .create_key()
            .key_spec(spec)
            .key_usage(usage)
            .send()
            .await
            .unwrap()
            .key_metadata()
            .unwrap()
            .key_id()
            .to_owned()
    }

    #[tokio::test]
    async fn data_keys_wrap_and_k256_keys_sign() {
        let Some(kms) = test_kms() else {
            eprintln!("LOCUS_TEST_KMS_ENDPOINT isn't set; skipping");
            return;
        };
        let sym = new_key(
            &kms,
            KeySpec::SymmetricDefault,
            KeyUsageType::EncryptDecrypt,
        )
        .await;
        let (plain, wrapped) = kms.generate_data_key(&sym).await.unwrap();
        assert_ne!(wrapped.as_slice(), plain.as_slice());
        assert_eq!(*kms.decrypt(&sym, &wrapped).await.unwrap(), *plain);
        let mut bad = wrapped.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(kms.decrypt(&sym, &bad).await.is_err());

        let k = new_key(&kms, KeySpec::EccSecgP256K1, KeyUsageType::SignVerify).await;
        let did = kms.k256_did_key(&k).await.unwrap();
        assert!(did.starts_with("did:key:zQ3s"), "{did}");
        for i in 0..20u8 {
            let msg = [b'm', i];
            let sig = kms.sign_k256(&k, &msg).await.unwrap();
            assert_eq!(sig.len(), 64);
            // Strict verification refuses high-S signatures, so every one
            // came back normalized.
            assert!(
                proto_blue_crypto::verify_signature(&did, &msg, &sig, false).unwrap(),
                "signature {i}"
            );
        }
        assert!(kms.k256_did_key(&sym).await.is_err(), "not a secp256k1 key");
    }
}
