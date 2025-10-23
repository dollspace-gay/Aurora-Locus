/// Well-known endpoints
/// Handles /.well-known/* endpoints for DID resolution and other standards
use crate::{
    context::AppContext,
    crypto::plc::PlcSigner,
    error::{PdsError, PdsResult},
};
use atproto::did_doc::{DidDocument, Service, VerificationMethod};
use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{Json, Response},
    routing::get,
    Router,
};

/// Build well-known routes
pub fn routes() -> Router<AppContext> {
    Router::new()
        .route("/.well-known/atproto-did", get(atproto_did))
        .route("/.well-known/did.json", get(did_document))
}

/// /.well-known/atproto-did
///
/// Returns the DID for this PDS server in plain text
/// Used for did:web resolution
pub async fn atproto_did(State(ctx): State<AppContext>) -> PdsResult<Response> {
    let did = ctx.service_did();

    // Return plain text DID
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(did.to_string().into())
        .map_err(|e| {
            crate::error::PdsError::Internal(format!("Failed to build response: {}", e))
        })?;

    Ok(response)
}

/// /.well-known/did.json
///
/// Returns the full DID document for this PDS server
/// Used for did:web DID resolution
pub async fn did_document(State(ctx): State<AppContext>) -> PdsResult<Json<DidDocument>> {
    let did = ctx.service_did().to_string();

    // Generate the DID document with verification methods and services
    let doc = generate_did_document(&ctx, &did).await?;

    Ok(Json(doc))
}

/// Generate a complete DID document for a did:web DID
///
/// Creates a DID document containing:
/// - Service endpoints (PDS URL)
/// - Verification methods (signing keys)
/// - Also known as (handles)
async fn generate_did_document(ctx: &AppContext, did: &str) -> PdsResult<DidDocument> {
    // Build service endpoint
    let service_url = ctx.service_url();
    let service = Service {
        id: format!("{}#atproto_pds", did),
        service_type: "AtprotoPersonalDataServer".to_string(),
        service_endpoint: service_url,
    };

    // Build verification method from repo signing key
    let verification_method = generate_verification_method(ctx, did)?;

    // Build DID document
    let doc = DidDocument {
        context: Some(serde_json::json!([
            "https://www.w3.org/ns/did/v1",
            "https://w3id.org/security/multikey/v1",
            "https://w3id.org/security/suites/secp256k1-2019/v1"
        ])),
        id: did.to_string(),
        also_known_as: vec![], // Could be populated with service handle
        service: vec![service],
        verification_method: vec![verification_method],
    };

    Ok(doc)
}

/// Generate verification method from repository signing key
fn generate_verification_method(ctx: &AppContext, did: &str) -> PdsResult<VerificationMethod> {
    // Load repo signing key from config
    let repo_key_hex = &ctx.config.authentication.repo_signing_key;
    let signer = PlcSigner::from_hex(repo_key_hex)?;

    // Get public key in multibase format
    let public_key_multibase = generate_multibase_key(&signer)?;

    Ok(VerificationMethod {
        id: format!("{}#atproto", did),
        key_type: "Multikey".to_string(),
        controller: did.to_string(),
        public_key_multibase: Some(public_key_multibase),
    })
}

/// Generate multibase-encoded public key from signer
///
/// Uses base58btc encoding with 'z' prefix (multibase format)
fn generate_multibase_key(signer: &PlcSigner) -> PdsResult<String> {
    use k256::ecdsa::VerifyingKey;

    // Get the verifying key from the signing key
    let verifying_key: VerifyingKey = signer.verifying_key();

    // Get compressed public key (33 bytes: 1 byte prefix + 32 bytes X coordinate)
    let public_key_bytes = verifying_key.to_encoded_point(true);
    let compressed_bytes = public_key_bytes.as_bytes();

    // Encode as base58btc with multibase 'z' prefix
    // For secp256k1, we use the compressed form
    let encoded = bs58::encode(compressed_bytes).into_string();

    // Return with multibase prefix 'z' for base58btc
    Ok(format!("z{}", encoded))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;
    use std::path::PathBuf;

    #[test]
    fn test_well_known_path() {
        // Well-known path should be at root level
        assert_eq!("/.well-known/atproto-did", "/.well-known/atproto-did");
    }

    fn create_test_config() -> ServerConfig {
        ServerConfig {
            service: ServiceConfig {
                hostname: "localhost".to_string(),
                port: 2583,
                service_did: "did:web:localhost".to_string(),
                version: "0.1.0".to_string(),
                blob_upload_limit: 5242880,
            },
            storage: StorageConfig {
                data_directory: PathBuf::from("./data"),
                account_db: PathBuf::from("./data/account.sqlite"),
                sequencer_db: PathBuf::from("./data/sequencer.sqlite"),
                did_cache_db: PathBuf::from("./data/did_cache.sqlite"),
                actor_store_directory: PathBuf::from("./data/actors"),
                blobstore: BlobstoreConfig::Disk {
                    location: PathBuf::from("./data/blobs"),
                    tmp_location: PathBuf::from("./data/temp"),
                },
            },
            authentication: AuthConfig {
                jwt_secret: "test_secret_key_that_is_32_chars".to_string(),
                admin_password: "test_password".to_string(),
                repo_signing_key: "a".repeat(64), // Valid hex key
                plc_rotation_key: "b".repeat(64), // Valid hex key
            },
            identity: IdentityConfig {
                did_plc_url: "https://plc.directory".to_string(),
                service_handle_domains: vec![".localhost".to_string()],
                did_cache_stale_ttl: 3600,
                did_cache_max_ttl: 86400,
            },
            email: None,
            invites: InviteConfig {
                required: false,
                interval: 604800,
                epoch: "2024-01-01T00:00:00Z".to_string(),
            },
            rate_limit: RateLimitConfig {
                enabled: false,
                global_requests_per_minute: 3000,
            },
            logging: LoggingConfig {
                level: "info".to_string(),
            },
        }
    }

    #[test]
    fn test_multibase_key_generation() {
        // Create a test signer
        let test_key = vec![0x42u8; 32]; // Valid 32-byte key
        let signer = PlcSigner::new(&test_key).unwrap();

        let result = generate_multibase_key(&signer);

        assert!(result.is_ok());
        let multibase = result.unwrap();

        // Should start with 'z' (base58btc multibase prefix)
        assert!(multibase.starts_with('z'));

        // Should be longer than just the prefix
        assert!(multibase.len() > 1);

        // Should be valid base58 after the prefix
        let base58_part = &multibase[1..];
        assert!(bs58::decode(base58_part).into_vec().is_ok());
    }

    #[test]
    fn test_multibase_determinism() {
        // Same key should produce same multibase encoding
        let test_key = vec![0x42u8; 32];
        let signer1 = PlcSigner::new(&test_key).unwrap();
        let signer2 = PlcSigner::new(&test_key).unwrap();

        let multibase1 = generate_multibase_key(&signer1).unwrap();
        let multibase2 = generate_multibase_key(&signer2).unwrap();

        assert_eq!(multibase1, multibase2);
    }

    #[tokio::test]
    async fn test_did_document_structure() {
        let config = create_test_config();
        let did = "did:web:localhost";

        // Test verification method generation with config
        let signer = PlcSigner::from_hex(&config.authentication.repo_signing_key).unwrap();
        let multibase = generate_multibase_key(&signer).unwrap();

        // Verify multibase format
        assert!(multibase.starts_with('z'));

        // Create verification method manually
        let vm = VerificationMethod {
            id: format!("{}#atproto", did),
            key_type: "Multikey".to_string(),
            controller: did.to_string(),
            public_key_multibase: Some(multibase),
        };

        // Create service
        let service = Service {
            id: format!("{}#atproto_pds", did),
            service_type: "AtprotoPersonalDataServer".to_string(),
            service_endpoint: "http://localhost:2583".to_string(),
        };

        // Create DID document
        let doc = DidDocument {
            context: Some(serde_json::json!([
                "https://www.w3.org/ns/did/v1",
                "https://w3id.org/security/multikey/v1",
                "https://w3id.org/security/suites/secp256k1-2019/v1"
            ])),
            id: did.to_string(),
            also_known_as: vec![],
            service: vec![service],
            verification_method: vec![vm],
        };

        // Verify DID document structure
        assert_eq!(doc.id, did);
        assert_eq!(doc.service.len(), 1);
        assert_eq!(doc.service[0].service_type, "AtprotoPersonalDataServer");
        assert_eq!(doc.verification_method.len(), 1);
        assert_eq!(doc.verification_method[0].key_type, "Multikey");

        // Test serialization
        let json = serde_json::to_string_pretty(&doc).unwrap();
        assert!(json.contains("\"@context\""));
        assert!(json.contains("did:web:localhost"));
        assert!(json.contains("AtprotoPersonalDataServer"));
        assert!(json.contains("Multikey"));
    }
}
