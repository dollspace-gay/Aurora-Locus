# OAuth 2.1 Integration Guide for Aurora Locus PDS

## Overview

Aurora Locus PDS implements OAuth 2.1 with ATProto-specific extensions for secure, standards-compliant authentication. This guide will help you integrate your application with our OAuth implementation.

## Features

- ✅ OAuth 2.1 Authorization Code Flow with PKCE
- ✅ DPoP (Demonstrating Proof-of-Possession) token binding
- ✅ Multi-device support with device management
- ✅ Refresh token rotation with replay detection
- ✅ Automatic token revocation on security events
- ✅ Fine-grained scope-based permissions

## Prerequisites

- Your application must be registered as an OAuth client (contact your PDS administrator)
- HTTPS endpoint for redirect URIs (required for production)
- Ability to generate and store cryptographic keys (for DPoP)

## Quick Start

### 1. Authorization Request

Initiate the OAuth flow by redirecting the user to the authorization endpoint:

```http
GET /oauth/authorize?
  response_type=code&
  client_id=YOUR_CLIENT_ID&
  redirect_uri=YOUR_REDIRECT_URI&
  scope=atproto&
  code_challenge=CODE_CHALLENGE&
  code_challenge_method=S256&
  state=RANDOM_STATE
```

**Parameters:**
- `response_type`: Must be `code`
- `client_id`: Your registered OAuth client ID
- `redirect_uri`: Where to redirect after authorization (must match registration)
- `scope`: Space-separated list of permissions (e.g., `atproto offline_access`)
- `code_challenge`: SHA-256 hash of your code_verifier (PKCE)
- `code_challenge_method`: Must be `S256`
- `state`: Random string for CSRF protection (recommended)

### 2. Generate PKCE Parameters

```javascript
// Generate code_verifier (random 43-128 character string)
const code_verifier = generateRandomString(128);

// Compute code_challenge = SHA256(code_verifier)
const code_challenge = base64url(sha256(code_verifier));
```

**Python Example:**
```python
import secrets
import hashlib
import base64

# Generate code_verifier
code_verifier = secrets.token_urlsafe(96)  # 128 chars

# Compute code_challenge
sha256_hash = hashlib.sha256(code_verifier.encode()).digest()
code_challenge = base64.urlsafe_b64encode(sha256_hash).decode().rstrip('=')
```

### 3. Handle Authorization Callback

After the user authorizes, they'll be redirected to your `redirect_uri` with a code:

```http
https://your-app.com/callback?code=AUTHORIZATION_CODE&state=RANDOM_STATE
```

**Verify:**
1. Check that `state` matches your original value (CSRF protection)
2. Extract the `code` parameter

### 4. Exchange Code for Tokens

Make a POST request to the token endpoint:

```http
POST /oauth/token
Content-Type: application/x-www-form-urlencoded
DPoP: DPOP_PROOF_JWT

grant_type=authorization_code&
code=AUTHORIZATION_CODE&
code_verifier=CODE_VERIFIER&
client_id=YOUR_CLIENT_ID&
redirect_uri=YOUR_REDIRECT_URI
```

**DPoP Header (Optional but Recommended):**

The DPoP header contains a signed JWT proving possession of a private key:

```json
{
  "typ": "dpop+jwt",
  "alg": "ES256",
  "jwk": {
    "kty": "EC",
    "crv": "P-256",
    "x": "...",
    "y": "..."
  }
}
.
{
  "jti": "unique-request-id",
  "htm": "POST",
  "htu": "https://pds.example.com/oauth/token",
  "iat": 1234567890
}
```

**Response:**
```json
{
  "access_token": "at_...",
  "refresh_token": "rt_...",
  "token_type": "DPoP",
  "expires_in": 3600,
  "scope": "atproto"
}
```

### 5. Use Access Token

Include the access token in API requests:

```http
GET /xrpc/com.atproto.repo.listRecords
Authorization: DPoP at_...
DPoP: DPOP_PROOF_JWT
```

**DPoP Proof for API Requests:**
```json
{
  "typ": "dpop+jwt",
  "alg": "ES256",
  "jwk": { ... }
}
.
{
  "jti": "unique-request-id",
  "htm": "GET",
  "htu": "https://pds.example.com/xrpc/com.atproto.repo.listRecords",
  "iat": 1234567890,
  "ath": "base64url(sha256(access_token))"
}
```

### 6. Refresh Tokens

When your access token expires, use the refresh token to get a new one:

```http
POST /oauth/token
Content-Type: application/x-www-form-urlencoded

grant_type=refresh_token&
refresh_token=rt_...&
client_id=YOUR_CLIENT_ID
```

**Important:** The server will issue a new refresh token and invalidate the old one (token rotation). Always store the new refresh token.

## Scopes

Available OAuth scopes:

| Scope | Description |
|-------|-------------|
| `atproto` | Full access to ATProto operations (default) |
| `atproto:read` | Read-only access to repositories |
| `atproto:write` | Write access to repositories |
| `atproto:admin` | Administrative operations |
| `offline_access` | Request refresh tokens |

## Security Best Practices

### 1. Always Use PKCE
- Generate a cryptographically random `code_verifier` (128 characters recommended)
- Use SHA-256 for `code_challenge`
- Never reuse code_verifier values

### 2. Implement DPoP
- Generate a new EC P-256 key pair for each device
- Sign DPoP proofs with the device private key
- Include `ath` claim in API requests (SHA-256 of access token)

### 3. Store Tokens Securely
- Never store tokens in localStorage or cookies
- Use secure, encrypted storage (e.g., OS keychain)
- Implement token expiration handling

### 4. Handle Token Rotation
- When refreshing tokens, immediately store the new refresh token
- Detect and handle refresh token replay attacks
- Implement exponential backoff for failed refresh attempts

### 5. Use State Parameter
- Generate a random `state` value for each authorization request
- Verify the state in the callback to prevent CSRF attacks

## Migration from JWT to OAuth

If you're currently using JWT authentication, follow these steps to migrate:

### Phase 1: Dual Support (Weeks 1-8)
1. Implement OAuth 2.1 flow alongside JWT
2. Test OAuth integration in development
3. Gradually roll out OAuth to test users

### Phase 2: Deprecation Warnings (Weeks 9-12)
1. Add deprecation warnings to JWT responses
2. Monitor OAuth adoption metrics
3. Communicate sunset timeline to users

### Phase 3: OAuth-Only (Week 13+)
1. Disable JWT authentication
2. Require OAuth for all new sessions
3. Provide fallback support for critical users

## Error Handling

### Common Errors

| Error Code | Description | Solution |
|------------|-------------|----------|
| `invalid_request` | Missing or invalid parameters | Check required parameters |
| `invalid_client` | Client ID mismatch or not registered | Verify client_id |
| `invalid_grant` | Authorization code expired or invalid | Request new authorization |
| `invalid_scope` | Requested scope not allowed | Request valid scopes |
| `pkce_verification_failed` | PKCE verifier doesn't match challenge | Check code_verifier calculation |

### Example Error Response

```json
{
  "error": "invalid_grant",
  "error_description": "Authorization code expired",
  "error_uri": "https://docs.atproto.com/errors/invalid_grant"
}
```

## Code Examples

### JavaScript/TypeScript (Node.js)

```typescript
import crypto from 'crypto';
import fetch from 'node-fetch';

// 1. Generate PKCE parameters
function generatePKCE() {
  const code_verifier = crypto.randomBytes(96).toString('base64url');
  const hash = crypto.createHash('sha256').update(code_verifier).digest();
  const code_challenge = hash.toString('base64url');
  return { code_verifier, code_challenge };
}

// 2. Build authorization URL
function getAuthorizationUrl(config: {
  pdsUrl: string;
  clientId: string;
  redirectUri: string;
  code_challenge: string;
}) {
  const params = new URLSearchParams({
    response_type: 'code',
    client_id: config.clientId,
    redirect_uri: config.redirectUri,
    scope: 'atproto offline_access',
    code_challenge: config.code_challenge,
    code_challenge_method: 'S256',
    state: crypto.randomBytes(16).toString('hex'),
  });

  return `${config.pdsUrl}/oauth/authorize?${params}`;
}

// 3. Exchange code for tokens
async function exchangeCodeForTokens(config: {
  pdsUrl: string;
  code: string;
  code_verifier: string;
  clientId: string;
  redirectUri: string;
}) {
  const response = await fetch(`${config.pdsUrl}/oauth/token`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/x-www-form-urlencoded',
    },
    body: new URLSearchParams({
      grant_type: 'authorization_code',
      code: config.code,
      code_verifier: config.code_verifier,
      client_id: config.clientId,
      redirect_uri: config.redirectUri,
    }),
  });

  if (!response.ok) {
    throw new Error(`Token exchange failed: ${await response.text()}`);
  }

  return await response.json();
}

// 4. Use tokens
async function makeApiRequest(config: {
  pdsUrl: string;
  endpoint: string;
  accessToken: string;
}) {
  const response = await fetch(`${config.pdsUrl}${config.endpoint}`, {
    headers: {
      'Authorization': `Bearer ${config.accessToken}`,
    },
  });

  return await response.json();
}
```

### Python

```python
import requests
import secrets
import hashlib
import base64
from urllib.parse import urlencode, urlparse, parse_qs

class OAuthClient:
    def __init__(self, pds_url, client_id, redirect_uri):
        self.pds_url = pds_url
        self.client_id = client_id
        self.redirect_uri = redirect_uri
        self.code_verifier = None

    def generate_pkce(self):
        """Generate PKCE code_verifier and code_challenge."""
        self.code_verifier = secrets.token_urlsafe(96)
        sha256_hash = hashlib.sha256(self.code_verifier.encode()).digest()
        code_challenge = base64.urlsafe_b64encode(sha256_hash).decode().rstrip('=')
        return code_challenge

    def get_authorization_url(self):
        """Build OAuth authorization URL."""
        code_challenge = self.generate_pkce()
        state = secrets.token_hex(16)

        params = {
            'response_type': 'code',
            'client_id': self.client_id,
            'redirect_uri': self.redirect_uri,
            'scope': 'atproto offline_access',
            'code_challenge': code_challenge,
            'code_challenge_method': 'S256',
            'state': state,
        }

        return f"{self.pds_url}/oauth/authorize?{urlencode(params)}", state

    def exchange_code(self, code):
        """Exchange authorization code for access and refresh tokens."""
        data = {
            'grant_type': 'authorization_code',
            'code': code,
            'code_verifier': self.code_verifier,
            'client_id': self.client_id,
            'redirect_uri': self.redirect_uri,
        }

        response = requests.post(
            f"{self.pds_url}/oauth/token",
            data=data,
            headers={'Content-Type': 'application/x-www-form-urlencoded'}
        )
        response.raise_for_status()
        return response.json()

    def refresh_tokens(self, refresh_token):
        """Refresh access token using refresh token."""
        data = {
            'grant_type': 'refresh_token',
            'refresh_token': refresh_token,
            'client_id': self.client_id,
        }

        response = requests.post(
            f"{self.pds_url}/oauth/token",
            data=data,
        )
        response.raise_for_status()
        return response.json()

# Usage
client = OAuthClient(
    pds_url='https://pds.example.com',
    client_id='your-client-id',
    redirect_uri='https://your-app.com/callback'
)

# Get authorization URL
auth_url, state = client.get_authorization_url()
print(f"Visit: {auth_url}")

# After user authorizes and you receive the callback...
# code = parse_qs(urlparse(callback_url).query)['code'][0]
# tokens = client.exchange_code(code)
# print(f"Access token: {tokens['access_token']}")
```

## Monitoring and Metrics

Your PDS exposes Prometheus metrics for OAuth operations:

- `oauth_authorization_requests_total` - Total authorization requests
- `oauth_token_exchanges_total` - Total token exchanges
- `oauth_token_rotations_total` - Total token rotations
- `oauth_dpop_verification_failures_total` - DPoP verification failures
- `oauth_pkce_verification_failures_total` - PKCE verification failures

## Support and Resources

- **Documentation:** https://docs.atproto.com/specs/oauth
- **Migration Guide:** See `jwt_sunset_date` header in JWT responses
- **Issue Tracker:** https://github.com/your-org/aurora-locus/issues
- **API Reference:** `/xrpc/_health` for server status

## Changelog

### v0.1.0 (Current)
- Initial OAuth 2.1 implementation
- PKCE required for all flows
- Optional DPoP support
- JWT fallback during transition period

### Planned Features
- Dynamic client registration (RFC 7591)
- Pushed Authorization Requests (PAR)
- Rich authorization requests
- JWT-secured authorization responses

---

**Note:** This implementation follows OAuth 2.1 (draft-ietf-oauth-v2-1-09) and ATProto OAuth specifications. Always use the latest client libraries when available.
