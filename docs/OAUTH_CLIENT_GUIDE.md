# OAuth 2.1 Client Authentication Guide

Complete guide for implementing OAuth 2.1 authentication with Aurora Locus PDS.

## Table of Contents

- [Overview](#overview)
- [Quick Start](#quick-start)
- [PKCE Flow](#pkce-flow)
- [DPoP Token Binding](#dpop-token-binding)
- [Complete Authentication Flow](#complete-authentication-flow)
- [Token Management](#token-management)
- [Device Management](#device-management)
- [Scope Reference](#scope-reference)
- [Migration from Legacy Auth](#migration-from-legacy-auth)
- [Best Practices](#best-practices)

## Overview

Aurora Locus implements **OAuth 2.1** with the following security features:

- **PKCE (Proof Key for Code Exchange)** - Mandatory for all clients (RFC 7636)
- **DPoP (Demonstrating Proof-of-Possession)** - Token binding to prevent token theft (RFC 9449)
- **Refresh Token Rotation** - Automatic rotation with replay detection
- **Multi-Device Support** - Separate tokens per device with granular revocation
- **Scope-Based Authorization** - Granular permissions for API access

### Why OAuth 2.1?

OAuth 2.1 consolidates best practices from OAuth 2.0 and removes insecure grant types:
- PKCE is **mandatory** (prevents authorization code interception)
- Refresh token rotation prevents replay attacks
- DPoP binds tokens to cryptographic keys
- Clear scope model for least-privilege access

## Quick Start

### 1. Generate PKCE Code Verifier

```rust
use rand::Rng;
use sha2::{Digest, Sha256};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

fn generate_pkce_pair() -> (String, String) {
    // Generate code_verifier (43-128 characters, URL-safe)
    let code_verifier: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(43)
        .map(char::from)
        .collect();

    // Generate code_challenge = base64url(SHA256(code_verifier))
    let mut hasher = Sha256::new();
    hasher.update(code_verifier.as_bytes());
    let hash = hasher.finalize();
    let code_challenge = URL_SAFE_NO_PAD.encode(hash);

    (code_verifier, code_challenge)
}
```

### 2. Initiate Authorization

```rust
let (code_verifier, code_challenge) = generate_pkce_pair();
let state = generate_random_string(32); // CSRF protection

let auth_url = format!(
    "https://pds.example.com/oauth/authorize?\
     response_type=code&\
     client_id={}&\
     redirect_uri={}&\
     scope={}&\
     code_challenge={}&\
     code_challenge_method=S256&\
     state={}",
    client_id,
    url_encode(redirect_uri),
    url_encode("atproto:repo.create atproto:read"),
    code_challenge,
    state
);

// Redirect user to auth_url in browser
```

### 3. Exchange Authorization Code for Tokens

```rust
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct TokenRequest {
    grant_type: String,
    code: String,
    code_verifier: String,
    client_id: String,
    redirect_uri: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    token_type: String, // "DPoP"
    expires_in: i64,
    scope: String,
}

async fn exchange_code(
    auth_code: &str,
    code_verifier: &str,
) -> Result<TokenResponse, Box<dyn std::error::Error>> {
    let client = Client::new();

    let response = client
        .post("https://pds.example.com/oauth/token")
        .form(&TokenRequest {
            grant_type: "authorization_code".to_string(),
            code: auth_code.to_string(),
            code_verifier: code_verifier.to_string(),
            client_id: "https://myapp.example.com/client-metadata.json".to_string(),
            redirect_uri: "https://myapp.example.com/callback".to_string(),
        })
        .send()
        .await?;

    Ok(response.json::<TokenResponse>().await?)
}
```

## PKCE Flow

PKCE (Proof Key for Code Exchange) protects against authorization code interception attacks.

### Code Verifier Requirements

Per RFC 7636:
- **Length**: 43-128 characters
- **Characters**: `[A-Z] / [a-z] / [0-9] / "-" / "." / "_" / "~"`
- **Entropy**: At least 256 bits (recommended: 43 characters)

### Code Challenge Methods

Aurora Locus **only** supports `S256` (SHA-256):

```
code_challenge = BASE64URL(SHA256(ASCII(code_verifier)))
```

Plain method is **not supported** per OAuth 2.1 spec.

### Example: Complete PKCE Implementation

```rust
use rand::Rng;
use sha2::{Digest, Sha256};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

pub struct PKCEPair {
    pub code_verifier: String,
    pub code_challenge: String,
}

impl PKCEPair {
    /// Generate a new PKCE code verifier and challenge pair
    pub fn generate() -> Self {
        // Generate 43-character URL-safe random string
        let code_verifier: String = rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(43)
            .map(char::from)
            .collect();

        // Compute SHA-256 hash
        let mut hasher = Sha256::new();
        hasher.update(code_verifier.as_bytes());
        let hash = hasher.finalize();

        // Encode as base64url (no padding)
        let code_challenge = URL_SAFE_NO_PAD.encode(hash);

        Self {
            code_verifier,
            code_challenge,
        }
    }

    /// Verify a code_verifier matches this challenge (for testing)
    pub fn verify(&self, code_verifier: &str) -> bool {
        let mut hasher = Sha256::new();
        hasher.update(code_verifier.as_bytes());
        let hash = hasher.finalize();
        let computed = URL_SAFE_NO_PAD.encode(hash);
        computed == self.code_challenge
    }
}

// Usage
let pkce = PKCEPair::generate();
println!("Verifier: {}", pkce.code_verifier);
println!("Challenge: {}", pkce.code_challenge);

// Store code_verifier securely until token exchange
// Send code_challenge in authorization request
```

### PKCE Security Notes

- **NEVER** send `code_verifier` in authorization request - only `code_challenge`
- **Store** `code_verifier` securely on client until token exchange
- **Destroy** `code_verifier` after successful token exchange
- **Use** cryptographically secure random number generator

## DPoP Token Binding

DPoP (Demonstrating Proof-of-Possession) binds access tokens to a cryptographic key pair, preventing token theft.

### Overview

- Client generates an **asymmetric key pair** (ECDSA P-256 recommended)
- Tokens are **bound** to the JWK thumbprint of the public key
- Client **proves possession** of private key on every API request
- Stolen tokens are **useless** without the private key

### Key Pair Generation

```rust
use jsonwebtoken::{Algorithm, EncodingKey, DecodingKey};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct DPoPKeyPair {
    pub private_key: String, // PEM format
    pub public_key: String,  // PEM format
    pub thumbprint: String,  // JWK thumbprint
}

impl DPoPKeyPair {
    /// Generate a new ECDSA P-256 key pair for DPoP
    pub fn generate() -> Result<Self, Box<dyn std::error::Error>> {
        use openssl::ec::{EcGroup, EcKey};
        use openssl::nid::Nid;
        use openssl::pkey::PKey;

        // Generate ECDSA P-256 key pair
        let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1)?;
        let ec_key = EcKey::generate(&group)?;
        let pkey = PKey::from_ec_key(ec_key)?;

        // Export to PEM
        let private_pem = pkey.private_key_to_pem_pkcs8()?;
        let public_pem = pkey.public_key_to_pem()?;

        // Compute JWK thumbprint (SHA-256 of canonical JWK)
        let thumbprint = compute_jwk_thumbprint(&public_pem)?;

        Ok(Self {
            private_key: String::from_utf8(private_pem)?,
            public_key: String::from_utf8(public_pem)?,
            thumbprint,
        })
    }
}

fn compute_jwk_thumbprint(public_pem: &[u8]) -> Result<String, Box<dyn std::error::Error>> {
    // Implementation details:
    // 1. Convert PEM to JWK
    // 2. Create canonical JSON representation (sorted keys, no whitespace)
    // 3. Compute SHA-256 hash
    // 4. Encode as base64url

    // This is a simplified placeholder - full implementation requires
    // proper JWK construction and canonicalization
    use sha2::{Digest, Sha256};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    let mut hasher = Sha256::new();
    hasher.update(public_pem);
    let hash = hasher.finalize();
    Ok(URL_SAFE_NO_PAD.encode(hash))
}
```

### Generating DPoP Proof

DPoP proof is a signed JWT sent in the `DPoP` HTTP header on every request.

```rust
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
struct DPoPProof {
    /// JWT ID (unique per request, prevents replay)
    jti: String,

    /// HTTP method (uppercase)
    htm: String,

    /// HTTP URI (without query or fragment)
    htu: String,

    /// Issued at (Unix timestamp)
    iat: i64,

    /// Access token hash (for bound tokens)
    #[serde(skip_serializing_if = "Option::is_none")]
    ath: Option<String>,
}

fn create_dpop_proof(
    private_key: &str,
    http_method: &str,
    http_uri: &str,
    access_token: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    // Create JWT header with JWK
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some("dpop+jwt".to_string());
    // header.jwk = Some(public_key_as_jwk); // Include public key in header

    // Create JWT claims
    let proof = DPoPProof {
        jti: Uuid::new_v4().to_string(),
        htm: http_method.to_uppercase(),
        htu: http_uri.to_string(),
        iat: chrono::Utc::now().timestamp(),
        ath: access_token.map(hash_access_token),
    };

    // Sign with private key
    let encoding_key = EncodingKey::from_ec_pem(private_key.as_bytes())?;
    let token = encode(&header, &proof, &encoding_key)?;

    Ok(token)
}

fn hash_access_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let hash = hasher.finalize();
    URL_SAFE_NO_PAD.encode(hash)
}
```

### Making DPoP-Authenticated Requests

```rust
async fn make_dpop_request(
    access_token: &str,
    dpop_key: &DPoPKeyPair,
    url: &str,
    method: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    use reqwest::Client;

    // Generate DPoP proof for this request
    let dpop_proof = create_dpop_proof(
        &dpop_key.private_key,
        method,
        url,
        Some(access_token),
    )?;

    // Make request with both Authorization and DPoP headers
    let client = Client::new();
    let response = client
        .request(method.parse()?, url)
        .header("Authorization", format!("DPoP {}", access_token))
        .header("DPoP", dpop_proof)
        .send()
        .await?;

    Ok(response.text().await?)
}
```

## Complete Authentication Flow

### Step-by-Step Implementation

```rust
use serde::{Deserialize, Serialize};

pub struct OAuthClient {
    client_id: String,
    redirect_uri: String,
    pds_url: String,
    pkce_verifier: Option<String>,
    dpop_key: Option<DPoPKeyPair>,
}

impl OAuthClient {
    pub fn new(client_id: String, redirect_uri: String, pds_url: String) -> Self {
        Self {
            client_id,
            redirect_uri,
            pds_url,
            pkce_verifier: None,
            dpop_key: None,
        }
    }

    /// Step 1: Generate authorization URL
    pub fn get_authorization_url(&mut self, scope: &str) -> Result<String, Box<dyn std::error::Error>> {
        // Generate PKCE pair
        let pkce = PKCEPair::generate();
        self.pkce_verifier = Some(pkce.code_verifier.clone());

        // Generate state for CSRF protection
        let state = generate_random_string(32);

        // Generate DPoP key pair
        self.dpop_key = Some(DPoPKeyPair::generate()?);

        let url = format!(
            "{}/oauth/authorize?\
             response_type=code&\
             client_id={}&\
             redirect_uri={}&\
             scope={}&\
             code_challenge={}&\
             code_challenge_method=S256&\
             state={}",
            self.pds_url,
            urlencoding::encode(&self.client_id),
            urlencoding::encode(&self.redirect_uri),
            urlencoding::encode(scope),
            pkce.code_challenge,
            state
        );

        Ok(url)
    }

    /// Step 2: Exchange authorization code for tokens
    pub async fn exchange_code(&mut self, code: &str) -> Result<TokenResponse, Box<dyn std::error::Error>> {
        let code_verifier = self.pkce_verifier
            .take()
            .ok_or("PKCE verifier not found - call get_authorization_url first")?;

        let client = reqwest::Client::new();

        let response = client
            .post(format!("{}/oauth/token", self.pds_url))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("code_verifier", &code_verifier),
                ("client_id", &self.client_id),
                ("redirect_uri", &self.redirect_uri),
            ])
            .send()
            .await?;

        let tokens: TokenResponse = response.json().await?;
        Ok(tokens)
    }

    /// Step 3: Make authenticated API request
    pub async fn make_request(
        &self,
        access_token: &str,
        endpoint: &str,
        method: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let dpop_key = self.dpop_key.as_ref()
            .ok_or("DPoP key not found")?;

        let url = format!("{}{}", self.pds_url, endpoint);
        make_dpop_request(access_token, dpop_key, &url, method).await
    }

    /// Step 4: Refresh access token
    pub async fn refresh_token(&self, refresh_token: &str) -> Result<TokenResponse, Box<dyn std::error::Error>> {
        let client = reqwest::Client::new();

        let response = client
            .post(format!("{}/oauth/token", self.pds_url))
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", &self.client_id),
            ])
            .send()
            .await?;

        Ok(response.json().await?)
    }
}

fn generate_random_string(length: usize) -> String {
    use rand::Rng;
    rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(length)
        .map(char::from)
        .collect()
}
```

### Usage Example

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize client
    let mut client = OAuthClient::new(
        "https://myapp.example.com/client-metadata.json".to_string(),
        "https://myapp.example.com/callback".to_string(),
        "https://pds.example.com".to_string(),
    );

    // Step 1: Get authorization URL
    let auth_url = client.get_authorization_url("atproto:repo.create atproto:read")?;
    println!("Visit: {}", auth_url);

    // User authorizes in browser and is redirected back with code
    let auth_code = "..."; // Extract from redirect URL

    // Step 2: Exchange code for tokens
    let tokens = client.exchange_code(auth_code).await?;
    println!("Access token: {}", tokens.access_token);

    // Step 3: Make authenticated requests
    let response = client.make_request(
        &tokens.access_token,
        "/xrpc/com.atproto.repo.createRecord",
        "POST",
    ).await?;

    println!("Response: {}", response);

    Ok(())
}
```

## Token Management

### Secure Token Storage

```rust
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64, // Unix timestamp
    pub scope: String,
    pub dpop_private_key: String, // PEM format
    pub dpop_public_key: String,
}

impl TokenSet {
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        now >= self.expires_at
    }

    pub fn needs_refresh(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        // Refresh 5 minutes before expiration
        now >= self.expires_at.saturating_sub(300)
    }
}
```

### Automatic Token Refresh

```rust
pub struct AuthenticatedClient {
    oauth_client: OAuthClient,
    tokens: TokenSet,
}

impl AuthenticatedClient {
    pub async fn make_request(
        &mut self,
        endpoint: &str,
        method: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        // Refresh if needed
        if self.tokens.needs_refresh() {
            let new_tokens = self.oauth_client
                .refresh_token(&self.tokens.refresh_token)
                .await?;

            self.update_tokens(new_tokens);
        }

        // Make request
        self.oauth_client.make_request(
            &self.tokens.access_token,
            endpoint,
            method,
        ).await
    }

    fn update_tokens(&mut self, response: TokenResponse) {
        let expires_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() + response.expires_in as u64;

        self.tokens.access_token = response.access_token;
        self.tokens.refresh_token = response.refresh_token;
        self.tokens.expires_at = expires_at;
        self.tokens.scope = response.scope;
    }
}
```

## Device Management

See [DEVICE_MANAGEMENT.md](DEVICE_MANAGEMENT.md) for complete device registration and management guide.

## Scope Reference

Complete scope hierarchy:

```
atproto:*                          # Full access (all scopes)
├── atproto:read                   # Read-only access
├── atproto:write                  # All write operations
│   ├── atproto:repo.*            # All repository operations
│   │   ├── atproto:repo.create   # Create records
│   │   ├── atproto:repo.update   # Update records
│   │   ├── atproto:repo.delete   # Delete records
│   │   ├── atproto:repo.get      # Get records
│   │   └── atproto:repo.list     # List records
│   ├── atproto:blob.upload       # Upload blobs
│   └── atproto:identity.update   # Update profile
├── atproto:admin.*               # Admin operations
└── ...
```

### Requesting Scopes

```rust
// Minimal scope (read-only)
let scope = "atproto:read";

// Create posts
let scope = "atproto:repo.create atproto:read";

// Full write access
let scope = "atproto:write atproto:read";

// Full access
let scope = "atproto:*";
```

## Migration from Legacy Auth

See [MIGRATION_GUIDE.md](MIGRATION_GUIDE.md) for step-by-step migration instructions.

## Best Practices

### Security

1. **Always use HTTPS** - Never send tokens over unencrypted connections
2. **Store tokens securely** - Use OS keychain/credential managers
3. **Implement token refresh** - Don't wait for 401 errors
4. **Validate redirect URIs** - Prevent authorization code interception
5. **Use state parameter** - Prevent CSRF attacks
6. **Rotate DPoP keys** - Generate new key pair per device
7. **Clear sensitive data** - Destroy code_verifier after token exchange

### Performance

1. **Cache DPoP proofs** - Reuse for same method/URI within validity window
2. **Refresh tokens early** - Don't wait until expiration
3. **Handle 401 errors** - Implement automatic refresh and retry

### User Experience

1. **Remember devices** - Use device tokens for seamless re-authentication
2. **Provide clear scope descriptions** - Let users understand permissions
3. **Implement device management** - Let users review and revoke devices
4. **Handle errors gracefully** - Provide clear messages for auth failures

### Error Handling

```rust
match client.make_request("/xrpc/com.atproto.repo.createRecord", "POST").await {
    Ok(response) => println!("Success: {}", response),
    Err(e) => {
        if e.to_string().contains("401") {
            // Token expired or revoked - refresh or re-authenticate
            println!("Authentication failed - please log in again");
        } else if e.to_string().contains("403") {
            // Insufficient scope
            println!("Permission denied - this operation requires additional permissions");
        } else {
            println!("Error: {}", e);
        }
    }
}
```

## Additional Resources

- [RFC 7636 - PKCE](https://datatracker.ietf.org/doc/html/rfc7636)
- [RFC 9449 - DPoP](https://datatracker.ietf.org/doc/html/rfc9449)
- [OAuth 2.1](https://datatracker.ietf.org/doc/html/draft-ietf-oauth-v2-1)
- [ATProto OAuth Spec](https://atproto.com/specs/oauth)
- [Device Management Guide](DEVICE_MANAGEMENT.md)
- [Migration Guide](MIGRATION_GUIDE.md)
- [API Reference](API_REFERENCE.md)
