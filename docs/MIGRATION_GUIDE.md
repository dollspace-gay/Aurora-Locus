# OAuth 2.1 Migration Guide

Complete guide for migrating from legacy session-based authentication to OAuth 2.1.

## Table of Contents

- [Overview](#overview)
- [Migration Timeline](#migration-timeline)
- [Before You Start](#before-you-start)
- [Step-by-Step Migration](#step-by-step-migration)
- [Backward Compatibility](#backward-compatibility)
- [Testing Your Migration](#testing-your-migration)
- [Troubleshooting](#troubleshooting)
- [FAQ](#faq)

## Overview

Aurora Locus is transitioning from legacy session-based authentication to OAuth 2.1 with DPoP token binding. This migration provides:

**Security Improvements:**
- PKCE prevents authorization code interception
- DPoP prevents access token theft
- Refresh token rotation prevents replay attacks
- Granular scope-based permissions

**User Experience:**
- Multi-device support with per-device tokens
- Seamless re-authentication on trusted devices
- Centralized device management and revocation

## Migration Timeline

### Phase 1: Dual Authentication (Current)

**Status:** Both legacy and OAuth authentication work simultaneously

- Legacy session tokens: ✅ **Fully supported**
- OAuth 2.1 tokens: ✅ **Fully supported**
- Scope enforcement: OAuth only (legacy has full access)

**Action Required:** None - your app continues working as-is

### Phase 2: OAuth Recommended (Estimated: Q2 2025)

- Legacy auth: ✅ Supported (deprecated warnings)
- OAuth 2.1: ✅ Recommended for new applications
- New features: OAuth-only

**Action Required:** Begin OAuth migration for new features

### Phase 3: OAuth Required (Estimated: Q4 2025)

- Legacy auth: ⚠️ Sunset notice (6 months)
- OAuth 2.1: ✅ Required for all applications

**Action Required:** Complete OAuth migration before sunset date

### Phase 4: OAuth Only (Estimated: Q2 2026)

- Legacy auth: ❌ Disabled
- OAuth 2.1: ✅ Only authentication method

**Action Required:** Must have completed OAuth migration

## Before You Start

### Prerequisites

1. **Understand OAuth 2.1 basics** - Read [OAUTH_CLIENT_GUIDE.md](OAUTH_CLIENT_GUIDE.md)
2. **Review your authentication flow** - Identify all places handling auth
3. **Plan token storage** - Decide how to store DPoP key pairs
4. **Consider multi-device support** - Plan device management UX

### What You'll Need

- **Client ID**: Your app's OAuth client identifier (URL to client metadata)
- **Redirect URI**: Where Aurora Locus redirects after authorization
- **Scopes**: What permissions your app needs
- **Secure storage**: For refresh tokens and DPoP private keys

### Dependency Updates

Add OAuth/crypto dependencies to your project:

```toml
[dependencies]
reqwest = { version = "0.11", features = ["json"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
jsonwebtoken = "9.0"
sha2 = "0.10"
base64 = "0.21"
rand = "0.8"
openssl = "0.10"  # For DPoP key generation
uuid = { version = "1.0", features = ["v4"] }
urlencoding = "2.1"
chrono = "0.4"
```

## Step-by-Step Migration

### Step 1: Add OAuth Client Helpers

Create a new module for OAuth authentication:

```rust
// src/auth/oauth_client.rs

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
    pub scope: String,
    pub dpop_private_key: String,
    pub dpop_public_key: String,
}

pub struct OAuthClient {
    client_id: String,
    redirect_uri: String,
    pds_url: String,
}

impl OAuthClient {
    pub fn new(client_id: String, redirect_uri: String, pds_url: String) -> Self {
        Self {
            client_id,
            redirect_uri,
            pds_url,
        }
    }

    // Add methods from OAUTH_CLIENT_GUIDE.md
}
```

### Step 2: Update Configuration

Add OAuth settings to your config:

```rust
// config.rs

#[derive(Debug, Clone)]
pub struct AppConfig {
    // Existing fields...

    // OAuth configuration
    pub oauth_client_id: String,
    pub oauth_redirect_uri: String,
    pub pds_url: String,
}

impl AppConfig {
    pub fn from_env() -> Self {
        Self {
            oauth_client_id: std::env::var("OAUTH_CLIENT_ID")
                .unwrap_or_else(|_| "https://myapp.example.com/client-metadata.json".to_string()),
            oauth_redirect_uri: std::env::var("OAUTH_REDIRECT_URI")
                .unwrap_or_else(|_| "https://myapp.example.com/callback".to_string()),
            pds_url: std::env::var("PDS_URL")
                .unwrap_or_else(|_| "https://pds.example.com".to_string()),
        }
    }
}
```

### Step 3: Implement Dual Authentication

Support both legacy and OAuth authentication simultaneously:

```rust
// src/auth/mod.rs

pub enum AuthToken {
    Legacy(String),  // Session token
    OAuth(TokenSet), // OAuth tokens
}

impl AuthToken {
    /// Get the authorization header value
    pub fn to_auth_header(&self) -> String {
        match self {
            AuthToken::Legacy(token) => format!("Bearer {}", token),
            AuthToken::OAuth(tokens) => format!("DPoP {}", tokens.access_token),
        }
    }

    /// Check if token needs refresh
    pub fn needs_refresh(&self) -> bool {
        match self {
            AuthToken::Legacy(_) => false, // Legacy tokens don't refresh
            AuthToken::OAuth(tokens) => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                now >= tokens.expires_at.saturating_sub(300) // 5 min buffer
            }
        }
    }
}

pub struct AuthManager {
    oauth_client: OAuthClient,
    current_token: Option<AuthToken>,
}

impl AuthManager {
    pub async fn make_request(
        &mut self,
        endpoint: &str,
        method: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        // Refresh OAuth token if needed
        if let Some(AuthToken::OAuth(tokens)) = &self.current_token {
            if tokens.needs_refresh() {
                self.refresh_oauth_token().await?;
            }
        }

        // Make request with appropriate auth
        match &self.current_token {
            Some(AuthToken::Legacy(token)) => {
                self.make_legacy_request(endpoint, method, token).await
            }
            Some(AuthToken::OAuth(tokens)) => {
                self.make_oauth_request(endpoint, method, tokens).await
            }
            None => Err("Not authenticated".into()),
        }
    }

    async fn make_legacy_request(
        &self,
        endpoint: &str,
        method: &str,
        token: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let client = reqwest::Client::new();
        let response = client
            .request(method.parse()?, format!("{}{}", self.oauth_client.pds_url, endpoint))
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await?;

        Ok(response.text().await?)
    }

    async fn make_oauth_request(
        &self,
        endpoint: &str,
        method: &str,
        tokens: &TokenSet,
    ) -> Result<String, Box<dyn std::error::Error>> {
        // Generate DPoP proof
        let dpop_proof = create_dpop_proof(
            &tokens.dpop_private_key,
            method,
            &format!("{}{}", self.oauth_client.pds_url, endpoint),
            Some(&tokens.access_token),
        )?;

        let client = reqwest::Client::new();
        let response = client
            .request(method.parse()?, format!("{}{}", self.oauth_client.pds_url, endpoint))
            .header("Authorization", format!("DPoP {}", tokens.access_token))
            .header("DPoP", dpop_proof)
            .send()
            .await?;

        Ok(response.text().await?)
    }

    async fn refresh_oauth_token(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(AuthToken::OAuth(tokens)) = &self.current_token {
            let new_tokens = self.oauth_client
                .refresh_token(&tokens.refresh_token)
                .await?;

            self.current_token = Some(AuthToken::OAuth(new_tokens));
        }

        Ok(())
    }
}
```

### Step 4: Add OAuth Login Flow

Create a new login flow alongside your existing one:

```rust
// src/auth/login.rs

pub enum LoginMethod {
    Legacy {
        username: String,
        password: String,
    },
    OAuth {
        authorization_code: String,
        code_verifier: String,
    },
}

pub async fn login(
    method: LoginMethod,
    oauth_client: &OAuthClient,
) -> Result<AuthToken, Box<dyn std::error::Error>> {
    match method {
        LoginMethod::Legacy { username, password } => {
            // Existing legacy login
            let token = perform_legacy_login(&username, &password).await?;
            Ok(AuthToken::Legacy(token))
        }
        LoginMethod::OAuth { authorization_code, code_verifier } => {
            // New OAuth login
            let tokens = oauth_client
                .exchange_code(&authorization_code, &code_verifier)
                .await?;
            Ok(AuthToken::OAuth(tokens))
        }
    }
}
```

### Step 5: Update UI/UX

Add OAuth login option to your login screen:

```rust
// In your UI code

pub fn show_login_screen() {
    // Existing "Login with Password" button
    if button("Login with Password") {
        // Show username/password form
        show_legacy_login_form();
    }

    // New "Login with OAuth" button
    if button("Login with OAuth (Recommended)") {
        start_oauth_flow();
    }
}

fn start_oauth_flow() {
    let mut oauth_client = OAuthClient::new(
        config.oauth_client_id.clone(),
        config.oauth_redirect_uri.clone(),
        config.pds_url.clone(),
    );

    // Generate PKCE and get authorization URL
    let auth_url = oauth_client
        .get_authorization_url("atproto:repo.create atproto:read")
        .unwrap();

    // Store PKCE verifier for later
    save_pkce_verifier(&oauth_client.pkce_verifier);

    // Open browser to authorization URL
    open::that(auth_url).unwrap();

    // Wait for callback with authorization code
    start_callback_server();
}
```

### Step 6: Handle OAuth Callback

Set up a callback handler for the redirect URI:

```rust
// src/auth/callback.rs

use axum::{
    extract::Query,
    response::Html,
    Router,
    routing::get,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct CallbackParams {
    code: String,
    state: String,
}

async fn oauth_callback(
    Query(params): Query<CallbackParams>,
) -> Html<&'static str> {
    // Verify state parameter (CSRF protection)
    // Exchange code for tokens
    // Store tokens securely
    // Redirect to app

    Html("<html><body>
        <h1>Authorization Successful!</h1>
        <p>You can close this window and return to the app.</p>
        <script>window.close();</script>
    </body></html>")
}

pub fn start_callback_server() {
    tokio::spawn(async {
        let app = Router::new()
            .route("/callback", get(oauth_callback));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
            .await
            .unwrap();

        axum::serve(listener, app).await.unwrap();
    });
}
```

### Step 7: Migrate Token Storage

Update your token storage to support both types:

```rust
// src/storage/token_storage.rs

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum StoredAuth {
    Legacy {
        token: String,
    },
    OAuth {
        access_token: String,
        refresh_token: String,
        expires_at: u64,
        scope: String,
        dpop_private_key: String,
        dpop_public_key: String,
    },
}

pub struct TokenStorage {
    storage_path: PathBuf,
}

impl TokenStorage {
    pub fn save(&self, auth: &StoredAuth) -> Result<(), Box<dyn std::error::Error>> {
        // Save to secure storage (keychain, encrypted file, etc.)
        let json = serde_json::to_string_pretty(auth)?;

        // For production, use OS-specific secure storage:
        // - macOS: Keychain
        // - Windows: Credential Manager
        // - Linux: Secret Service API / gnome-keyring

        std::fs::write(&self.storage_path, json)?;
        Ok(())
    }

    pub fn load(&self) -> Result<Option<StoredAuth>, Box<dyn std::error::Error>> {
        if !self.storage_path.exists() {
            return Ok(None);
        }

        let json = std::fs::read_to_string(&self.storage_path)?;
        let auth = serde_json::from_str(&json)?;
        Ok(Some(auth))
    }

    pub fn clear(&self) -> Result<(), Box<dyn std::error::Error>> {
        if self.storage_path.exists() {
            std::fs::remove_file(&self.storage_path)?;
        }
        Ok(())
    }
}
```

### Step 8: Test Both Authentication Methods

Create integration tests for both flows:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_legacy_auth_still_works() {
        let mut auth_manager = AuthManager::new_with_legacy_token("existing_token");

        let response = auth_manager
            .make_request("/xrpc/com.atproto.repo.getRecord", "GET")
            .await;

        assert!(response.is_ok());
    }

    #[tokio::test]
    async fn test_oauth_auth_works() {
        let mut auth_manager = AuthManager::new_with_oauth_tokens(test_tokens());

        let response = auth_manager
            .make_request("/xrpc/com.atproto.repo.createRecord", "POST")
            .await;

        assert!(response.is_ok());
    }

    #[tokio::test]
    async fn test_automatic_token_refresh() {
        let mut auth_manager = AuthManager::new_with_oauth_tokens(expiring_tokens());

        // First request should trigger refresh
        let response1 = auth_manager
            .make_request("/xrpc/com.atproto.repo.getRecord", "GET")
            .await;

        assert!(response1.is_ok());

        // Second request should use new token
        let response2 = auth_manager
            .make_request("/xrpc/com.atproto.repo.getRecord", "GET")
            .await;

        assert!(response2.is_ok());
    }
}
```

## Backward Compatibility

Aurora Locus maintains backward compatibility during the transition:

### Legacy Session Tokens

- ✅ **Continue working** on all endpoints
- ✅ **No scope restrictions** (full access like before)
- ✅ **No expiration changes** (same TTL)
- ⚠️ **Deprecated** (will be removed in future)

### Mixed Environment

Your app can use **both** authentication methods:
- Different users can use different methods
- Same user can have both legacy and OAuth tokens
- Gradual rollout: OAuth for new users, legacy for existing

### API Compatibility

All existing API endpoints work with both auth types:

```http
# Legacy auth (still works)
GET /xrpc/com.atproto.repo.getRecord
Authorization: Bearer <session_token>

# OAuth auth (new method)
GET /xrpc/com.atproto.repo.createRecord
Authorization: DPoP <access_token>
DPoP: <dpop_proof_jwt>
```

### Feature Flags

Use feature flags to control rollout:

```rust
pub struct AppConfig {
    pub enable_oauth: bool,
    pub force_oauth_for_new_users: bool,
    pub legacy_auth_sunset_date: Option<DateTime<Utc>>,
}

impl AppConfig {
    pub fn should_use_oauth(&self) -> bool {
        self.enable_oauth && (
            self.force_oauth_for_new_users ||
            self.is_past_sunset_warning()
        )
    }

    pub fn is_past_sunset_warning(&self) -> bool {
        if let Some(sunset) = self.legacy_auth_sunset_date {
            let warning = sunset - Duration::days(180); // 6 months warning
            Utc::now() > warning
        } else {
            false
        }
    }
}
```

## Testing Your Migration

### Manual Testing Checklist

- [ ] OAuth login flow completes successfully
- [ ] Can create records with OAuth token
- [ ] Can read records with OAuth token
- [ ] Token refresh happens automatically
- [ ] Legacy login still works (if supported)
- [ ] Legacy tokens still make successful API calls
- [ ] DPoP proof generation works correctly
- [ ] Scope errors return 403 with clear message
- [ ] Expired tokens trigger refresh
- [ ] Revoked tokens fail with 401

### Automated Testing

```rust
#[cfg(test)]
mod integration_tests {
    #[tokio::test]
    async fn test_full_oauth_flow() {
        // 1. Generate PKCE pair
        let (verifier, challenge) = generate_pkce_pair();

        // 2. Get authorization URL
        let auth_url = oauth_client.get_authorization_url(
            "atproto:repo.create atproto:read"
        ).unwrap();
        assert!(auth_url.contains(&challenge));

        // 3. Simulate user authorization (requires test PDS)
        let auth_code = simulate_user_authorization(&auth_url).await;

        // 4. Exchange code for tokens
        let tokens = oauth_client
            .exchange_code(&auth_code, &verifier)
            .await
            .unwrap();

        assert!(!tokens.access_token.is_empty());
        assert!(!tokens.refresh_token.is_empty());

        // 5. Make authenticated request
        let response = oauth_client
            .make_request(
                &tokens.access_token,
                "/xrpc/com.atproto.repo.createRecord",
                "POST"
            )
            .await;

        assert!(response.is_ok());
    }
}
```

## Troubleshooting

### Common Issues

#### Issue: "Invalid code_challenge"

**Cause:** PKCE challenge not computed correctly

**Solution:**
```rust
// Ensure using SHA-256 and base64url (NO padding)
use sha2::{Digest, Sha256};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

let mut hasher = Sha256::new();
hasher.update(code_verifier.as_bytes());
let hash = hasher.finalize();
let challenge = URL_SAFE_NO_PAD.encode(hash); // NO padding!
```

#### Issue: "DPoP proof validation failed"

**Cause:** DPoP proof missing required claims or incorrect signature

**Solution:**
```rust
// Ensure all required claims are present:
{
    "jti": "unique-nonce",  // Must be unique per request
    "htm": "POST",          // HTTP method (uppercase)
    "htu": "https://pds.example.com/xrpc/...",  // Full URL without query
    "iat": 1234567890,      // Current timestamp
    "ath": "..."            // Access token hash (when bound)
}

// Ensure JWT header includes JWK:
{
    "typ": "dpop+jwt",
    "alg": "ES256",
    "jwk": { /* public key */ }
}
```

#### Issue: "Token refresh fails"

**Cause:** Refresh token expired or revoked

**Solution:**
- Check refresh token expiration (typically 30-90 days)
- Implement re-authentication flow when refresh fails
- Store device tokens for seamless re-auth

#### Issue: "Scope insufficient"

**Cause:** OAuth token doesn't have required scope

**Solution:**
```rust
// Request sufficient scopes during authorization
let scope = "atproto:repo.create atproto:repo.update atproto:read";

// Handle 403 errors by re-authorizing with additional scopes
if error.status() == 403 {
    let new_scope = "atproto:write atproto:read"; // Request broader scope
    reauthorize_with_scope(new_scope).await?;
}
```

### Debug Logging

Enable debug logging to troubleshoot issues:

```rust
// Enable in development
env::set_var("RUST_LOG", "aurora_locus=debug,oauth_client=trace");
tracing_subscriber::fmt::init();

// Log all HTTP requests
let client = reqwest::Client::builder()
    .connection_verbose(true)
    .build()?;
```

## FAQ

**Q: Do I need to migrate all users at once?**

A: No! You can support both legacy and OAuth authentication simultaneously. Migrate users gradually.

**Q: What happens to existing session tokens?**

A: They continue working until sunset date (estimated Q2 2026). No immediate action required.

**Q: Can I use OAuth without DPoP?**

A: No, DPoP is mandatory for OAuth 2.1 in Aurora Locus for security reasons.

**Q: How do I handle multiple devices?**

A: Each device should have its own OAuth token with unique DPoP key pair. See [DEVICE_MANAGEMENT.md](DEVICE_MANAGEMENT.md).

**Q: What scopes should I request?**

A: Request the **minimum** scopes needed. Start with `atproto:read` and add write scopes as needed.

**Q: How long do OAuth tokens last?**

A: Access tokens: 1 hour, Refresh tokens: 30-90 days (configurable). Use automatic refresh to handle expiration.

**Q: Can I test OAuth without a real PDS?**

A: Yes! Use a local Aurora Locus instance or the public test PDS at `https://test.aurora-locus.dev`.

**Q: What if OAuth authorization fails?**

A: Show a clear error message and provide fallback to legacy login (during transition period).

## Next Steps

1. **Read** [OAUTH_CLIENT_GUIDE.md](OAUTH_CLIENT_GUIDE.md) for complete implementation details
2. **Implement** OAuth login flow in a feature branch
3. **Test** thoroughly with both auth methods
4. **Deploy** with feature flag (OAuth optional)
5. **Monitor** OAuth adoption rate
6. **Communicate** sunset timeline to users
7. **Deprecate** legacy auth after sufficient adoption

## Additional Resources

- [OAuth Client Guide](OAUTH_CLIENT_GUIDE.md)
- [Device Management](DEVICE_MANAGEMENT.md)
- [API Reference](API_REFERENCE.md)
- [Security Best Practices](SECURITY.md)
- [Example Implementation](examples/oauth_migration/)

## Support

For migration assistance:
- GitHub Issues: https://github.com/aurora-locus/aurora-locus/issues
- Discord: https://discord.gg/aurora-locus
- Email: support@aurora-locus.dev
