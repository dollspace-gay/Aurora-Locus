//! Server-Side Authentication and Account Management for PDS
//!
//! This module implements the server-side authentication system for a Personal Data Server (PDS).
//! It provides:
//!
//! - Account creation and management
//! - Password hashing and verification (Argon2)
//! - JWT token generation and validation
//! - Session management
//! - Authentication endpoints
//!
//! # Security Features
//!
//! - Argon2id password hashing (OWASP recommended)
//! - Short-lived access tokens (~1 hour)
//! - Long-lived refresh tokens (~6 months)
//! - RS256 JWT signing
//! - Session storage with expiration
//! - Rate limiting support (implementation-dependent)
//!
//! # Example
//!
//! ```no_run
//! use atproto::server_auth::{AuthManager, AccountCreate, PasswordHasher};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create auth manager
//! let mut auth_manager = AuthManager::new();
//!
//! // Create account
//! let create_req = AccountCreate {
//!     handle: "alice.bsky.social".to_string(),
//!     email: Some("alice@example.com".to_string()),
//!     password: Some("secure_password_123".to_string()),
//!     invite_code: None,
//!     did: None,
//!     verification_code: None,
//!     verification_phone: None,
//!     recovery_key: None,
//!     plc_op: None,
//! };
//!
//! let account = auth_manager.create_account(create_req).await?;
//! println!("Created account: {}", account.did);
//!
//! // Create session (login)
//! let session = auth_manager.create_session(
//!     "alice.bsky.social",
//!     "secure_password_123",
//!     None,
//! ).await?;
//!
//! println!("Access token: {}", session.access_jwt);
//! # Ok(())
//! # }
//! ```

use crate::types::Did;
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher as _, PasswordVerifier, SaltString},
    Argon2,
};
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use thiserror::Error;

/// Authentication error types
#[derive(Error, Debug)]
pub enum AuthError {
    #[error("Invalid credentials")]
    InvalidCredentials,

    #[error("Account not found")]
    AccountNotFound,

    #[error("Handle not available: {0}")]
    HandleNotAvailable(String),

    #[error("Invalid handle: {0}")]
    InvalidHandle(String),

    #[error("Invalid password: {0}")]
    InvalidPassword(String),

    #[error("Invalid invite code")]
    InvalidInviteCode,

    #[error("Invite code required")]
    InviteCodeRequired,

    #[error("Password required")]
    PasswordRequired,

    #[error("Token expired")]
    TokenExpired,

    #[error("Invalid token")]
    InvalidToken,

    #[error("Account taken down")]
    AccountTakedown,

    #[error("Account suspended")]
    AccountSuspended,

    #[error("Account deactivated")]
    AccountDeactivated,

    #[error("Auth factor token required")]
    AuthFactorTokenRequired,

    #[error("Password hashing failed: {0}")]
    PasswordHashError(String),

    #[error("JWT error: {0}")]
    JwtError(#[from] jsonwebtoken::errors::Error),

    #[error("Unsupported domain: {0}")]
    UnsupportedDomain(String),

    #[error("Unresolvable DID: {0}")]
    UnresolvableDid(String),

    #[error("Incompatible DID document")]
    IncompatibleDidDoc,
}

/// Result type for authentication operations
pub type Result<T> = std::result::Result<T, AuthError>;

/// Account status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AccountStatus {
    Takendown,
    Suspended,
    Deactivated,
}

/// Account data stored in database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// Account DID
    pub did: Did,

    /// Account handle
    pub handle: String,

    /// Email address (optional)
    pub email: Option<String>,

    /// Email confirmed
    pub email_confirmed: bool,

    /// Email-based 2FA enabled
    pub email_auth_factor: bool,

    /// Password hash (Argon2)
    pub password_hash: String,

    /// Account active
    pub active: bool,

    /// Account status (if not active)
    pub status: Option<AccountStatus>,

    /// Created at timestamp
    pub created_at: DateTime<Utc>,

    /// Updated at timestamp
    pub updated_at: DateTime<Utc>,
}

/// Account creation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountCreate {
    /// Requested handle
    pub handle: String,

    /// Email address (optional)
    pub email: Option<String>,

    /// Password (optional but needed for password-based auth)
    pub password: Option<String>,

    /// Invite code (may be required on some instances)
    pub invite_code: Option<String>,

    /// Pre-existing DID (for account import)
    pub did: Option<String>,

    /// Verification code
    pub verification_code: Option<String>,

    /// Verification phone
    pub verification_phone: Option<String>,

    /// DID PLC recovery key
    pub recovery_key: Option<String>,

    /// Signed DID PLC operation (for import)
    pub plc_op: Option<serde_json::Value>,
}

/// Account creation response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountCreated {
    /// Access JWT token
    pub access_jwt: String,

    /// Refresh JWT token
    pub refresh_jwt: String,

    /// Account handle
    pub handle: String,

    /// Account DID
    pub did: String,

    /// DID document (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub did_doc: Option<serde_json::Value>,
}

/// Session creation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCreate {
    /// Identifier (handle, email, or DID)
    pub identifier: String,

    /// Password
    pub password: String,

    /// Auth factor token (for 2FA)
    pub auth_factor_token: Option<String>,

    /// Allow takendown accounts
    #[serde(default)]
    pub allow_takendown: bool,
}

/// Session data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Access JWT token (short-lived, ~1 hour)
    pub access_jwt: String,

    /// Refresh JWT token (long-lived, ~6 months)
    pub refresh_jwt: String,

    /// Account handle
    pub handle: String,

    /// Account DID
    pub did: String,

    /// DID document (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub did_doc: Option<serde_json::Value>,

    /// Email (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,

    /// Email confirmed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email_confirmed: Option<bool>,

    /// Email auth factor enabled
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email_auth_factor: Option<bool>,

    /// Account active
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,

    /// Account status
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<AccountStatus>,
}

/// JWT claims for access/refresh tokens
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenClaims {
    /// Issuer (PDS URL)
    pub iss: String,

    /// Subject (user DID)
    pub sub: String,

    /// Audience
    pub aud: String,

    /// Issued at (Unix timestamp)
    pub iat: i64,

    /// Expires at (Unix timestamp)
    pub exp: i64,

    /// Token type ("access" or "refresh")
    #[serde(rename = "type")]
    pub token_type: String,
}

/// Password hasher using Argon2id
pub struct PasswordHasher;

impl PasswordHasher {
    /// Hash a password using Argon2id
    pub fn hash(password: &str) -> Result<String> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();

        argon2
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|e| AuthError::PasswordHashError(e.to_string()))
    }

    /// Verify a password against a hash
    pub fn verify(password: &str, hash: &str) -> Result<bool> {
        let parsed_hash = PasswordHash::new(hash)
            .map_err(|e| AuthError::PasswordHashError(e.to_string()))?;

        let argon2 = Argon2::default();

        Ok(argon2
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_ok())
    }
}

/// JWT token manager
pub struct TokenManager {
    /// Issuer (PDS URL)
    issuer: String,

    /// Audience
    audience: String,

    /// Encoding key (private key for signing)
    encoding_key: EncodingKey,

    /// Decoding key (public key for verification)
    decoding_key: DecodingKey,

    /// Access token expiration (default: 1 hour)
    access_token_duration: Duration,

    /// Refresh token expiration (default: 6 months)
    refresh_token_duration: Duration,
}

impl TokenManager {
    /// Create a new token manager with provided RSA PEM keys
    ///
    /// # Arguments
    /// * `issuer` - The token issuer (typically the PDS URL)
    /// * `audience` - The intended token audience
    /// * `private_key_pem` - RSA private key in PEM format
    /// * `public_key_pem` - RSA public key in PEM format
    pub fn from_keys(
        issuer: String,
        audience: String,
        private_key_pem: &[u8],
        public_key_pem: &[u8],
    ) -> Result<Self> {
        Ok(Self {
            issuer,
            audience,
            encoding_key: EncodingKey::from_rsa_pem(private_key_pem)?,
            decoding_key: DecodingKey::from_rsa_pem(public_key_pem)?,
            access_token_duration: Duration::hours(1),
            refresh_token_duration: Duration::days(180), // ~6 months
        })
    }

    /// Create a new token manager with test keys (for testing only)
    ///
    /// In production, use `from_keys()` with keys loaded from secure storage.
    #[cfg(test)]
    pub fn new(issuer: String, audience: String) -> Self {
        // For testing, load test keys
        let private_key = include_bytes!("../test_keys/private_key.pem");
        let public_key = include_bytes!("../test_keys/public_key.pem");

        Self::from_keys(issuer, audience, private_key, public_key).unwrap()
    }

    /// Generate an access token
    pub fn generate_access_token(&self, did: &str) -> Result<String> {
        let now = Utc::now();
        let exp = now + self.access_token_duration;

        let claims = TokenClaims {
            iss: self.issuer.clone(),
            sub: did.to_string(),
            aud: self.audience.clone(),
            iat: now.timestamp(),
            exp: exp.timestamp(),
            token_type: "access".to_string(),
        };

        let header = Header::new(Algorithm::RS256);
        encode(&header, &claims, &self.encoding_key).map_err(AuthError::from)
    }

    /// Generate a refresh token
    pub fn generate_refresh_token(&self, did: &str) -> Result<String> {
        let now = Utc::now();
        let exp = now + self.refresh_token_duration;

        let claims = TokenClaims {
            iss: self.issuer.clone(),
            sub: did.to_string(),
            aud: self.audience.clone(),
            iat: now.timestamp(),
            exp: exp.timestamp(),
            token_type: "refresh".to_string(),
        };

        let header = Header::new(Algorithm::RS256);
        encode(&header, &claims, &self.encoding_key).map_err(AuthError::from)
    }

    /// Validate and decode a token
    pub fn validate_token(&self, token: &str, expected_type: &str) -> Result<TokenClaims> {
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[&self.audience]);
        validation.set_issuer(&[&self.issuer]);

        let token_data = decode::<TokenClaims>(token, &self.decoding_key, &validation)?;

        // Check token type
        if token_data.claims.token_type != expected_type {
            return Err(AuthError::InvalidToken);
        }

        // Check expiration
        let now = Utc::now().timestamp();
        if token_data.claims.exp < now {
            return Err(AuthError::TokenExpired);
        }

        Ok(token_data.claims)
    }
}

/// Session data: (did, expiry)
type SessionData = (String, DateTime<Utc>);

/// Session map type
type SessionMap = HashMap<String, SessionData>;

/// Session storage (in-memory)
///
/// In production, this should be backed by a database or Redis.
#[derive(Clone)]
pub struct SessionStore {
    /// Sessions by access token
    sessions: Arc<RwLock<SessionMap>>, // token -> (did, expiry)
}

impl SessionStore {
    /// Create a new session store
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Store a session
    pub fn store(&self, token: &str, did: &str, expiry: DateTime<Utc>) {
        let mut sessions = self.sessions.write().unwrap();
        sessions.insert(token.to_string(), (did.to_string(), expiry));
    }

    /// Get a session
    pub fn get(&self, token: &str) -> Option<String> {
        let sessions = self.sessions.read().unwrap();
        sessions.get(token).and_then(|(did, expiry)| {
            if *expiry > Utc::now() {
                Some(did.clone())
            } else {
                None
            }
        })
    }

    /// Delete a session
    pub fn delete(&self, token: &str) {
        let mut sessions = self.sessions.write().unwrap();
        sessions.remove(token);
    }

    /// Clean up expired sessions
    pub fn cleanup_expired(&self) {
        let mut sessions = self.sessions.write().unwrap();
        let now = Utc::now();
        sessions.retain(|_, (_, expiry)| *expiry > now);
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Authentication manager
///
/// This is the main entry point for server-side authentication.
pub struct AuthManager {
    /// Account storage (DID -> Account)
    accounts_by_did: Arc<RwLock<HashMap<String, Account>>>,

    /// Account lookup by handle
    accounts_by_handle: Arc<RwLock<HashMap<String, String>>>, // handle -> did

    /// Token manager
    token_manager: TokenManager,

    /// Session store
    session_store: SessionStore,

    /// Require invite codes
    pub require_invite_code: bool,

    /// Valid invite codes
    invite_codes: Arc<RwLock<HashMap<String, InviteCode>>>,
}

/// Invite code data
#[derive(Debug, Clone)]
pub struct InviteCode {
    pub code: String,
    pub available: usize,
    pub disabled: bool,
    pub created_at: DateTime<Utc>,
}

impl AuthManager {
    /// Create a new authentication manager (for testing only)
    ///
    /// In production, create an AuthManager with proper keys using the builder pattern.
    #[cfg(test)]
    pub fn new() -> Self {
        Self {
            accounts_by_did: Arc::new(RwLock::new(HashMap::new())),
            accounts_by_handle: Arc::new(RwLock::new(HashMap::new())),
            token_manager: TokenManager::new(
                "https://pds.example.com".to_string(),
                "atproto".to_string(),
            ),
            session_store: SessionStore::new(),
            require_invite_code: false,
            invite_codes: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new account
    pub async fn create_account(&mut self, req: AccountCreate) -> Result<AccountCreated> {
        // Validate handle
        if !crate::handle::is_valid_handle(&req.handle) {
            return Err(AuthError::InvalidHandle(req.handle));
        }

        // Check if handle is available
        {
            let handles = self.accounts_by_handle.read().unwrap();
            if handles.contains_key(&req.handle) {
                return Err(AuthError::HandleNotAvailable(req.handle));
            }
        }

        // Check invite code if required
        if self.require_invite_code {
            if let Some(code) = &req.invite_code {
                self.validate_and_use_invite_code(code)?;
            } else {
                return Err(AuthError::InviteCodeRequired);
            }
        }

        // Password is required for password-based auth
        let password = req.password.ok_or(AuthError::PasswordRequired)?;

        // Hash password
        let password_hash = PasswordHasher::hash(&password)?;

        // Generate DID (in production, this would use DID PLC)
        let did = if let Some(existing_did) = req.did {
            Did::new(&existing_did).map_err(|_| AuthError::UnresolvableDid(existing_did))?
        } else {
            // Generate new DID (simplified - in production use DID PLC)
            let did_str = format!("did:plc:{}", uuid::Uuid::new_v4().to_string().replace("-", ""));
            Did::new(&did_str).unwrap()
        };

        // Create account
        let account = Account {
            did: did.clone(),
            handle: req.handle.clone(),
            email: req.email,
            email_confirmed: false,
            email_auth_factor: false,
            password_hash,
            active: true,
            status: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        // Store account
        {
            let mut accounts = self.accounts_by_did.write().unwrap();
            accounts.insert(did.to_string(), account.clone());
        }
        {
            let mut handles = self.accounts_by_handle.write().unwrap();
            handles.insert(req.handle.clone(), did.to_string());
        }

        // Generate tokens
        let access_jwt = self.token_manager.generate_access_token(did.as_str())?;
        let refresh_jwt = self.token_manager.generate_refresh_token(did.as_str())?;

        // Store session
        let expiry = Utc::now() + Duration::hours(1);
        self.session_store.store(&access_jwt, did.as_str(), expiry);

        Ok(AccountCreated {
            access_jwt,
            refresh_jwt,
            handle: account.handle,
            did: did.to_string(),
            did_doc: None,
        })
    }

    /// Create a session (login)
    pub async fn create_session(
        &self,
        identifier: &str,
        password: &str,
        auth_factor_token: Option<String>,
    ) -> Result<Session> {
        // Find account by identifier (handle, email, or DID)
        let account = self.find_account_by_identifier(identifier)?;

        // Check account status
        if !account.active {
            return match account.status {
                Some(AccountStatus::Takendown) => Err(AuthError::AccountTakedown),
                Some(AccountStatus::Suspended) => Err(AuthError::AccountSuspended),
                Some(AccountStatus::Deactivated) => Err(AuthError::AccountDeactivated),
                None => Err(AuthError::AccountNotFound),
            };
        }

        // Verify password
        if !PasswordHasher::verify(password, &account.password_hash)? {
            return Err(AuthError::InvalidCredentials);
        }

        // Check 2FA if enabled
        if account.email_auth_factor && auth_factor_token.is_none() {
            return Err(AuthError::AuthFactorTokenRequired);
        }

        // Generate tokens
        let access_jwt = self.token_manager.generate_access_token(account.did.as_str())?;
        let refresh_jwt = self.token_manager.generate_refresh_token(account.did.as_str())?;

        // Store session
        let expiry = Utc::now() + Duration::hours(1);
        self.session_store.store(&access_jwt, account.did.as_str(), expiry);

        Ok(Session {
            access_jwt,
            refresh_jwt,
            handle: account.handle.clone(),
            did: account.did.to_string(),
            did_doc: None,
            email: account.email.clone(),
            email_confirmed: Some(account.email_confirmed),
            email_auth_factor: Some(account.email_auth_factor),
            active: Some(account.active),
            status: account.status.clone(),
        })
    }

    /// Refresh a session
    pub async fn refresh_session(&self, refresh_token: &str) -> Result<Session> {
        // Validate refresh token
        let claims = self.token_manager.validate_token(refresh_token, "refresh")?;

        // Get account
        let account = {
            let accounts = self.accounts_by_did.read().unwrap();
            accounts
                .get(&claims.sub)
                .cloned()
                .ok_or(AuthError::AccountNotFound)?
        };

        // Check account status
        if !account.active {
            return Err(AuthError::AccountTakedown);
        }

        // Generate new tokens
        let access_jwt = self.token_manager.generate_access_token(account.did.as_str())?;
        let refresh_jwt = self.token_manager.generate_refresh_token(account.did.as_str())?;

        // Store session
        let expiry = Utc::now() + Duration::hours(1);
        self.session_store.store(&access_jwt, account.did.as_str(), expiry);

        Ok(Session {
            access_jwt,
            refresh_jwt,
            handle: account.handle.clone(),
            did: account.did.to_string(),
            did_doc: None,
            email: None,
            email_confirmed: None,
            email_auth_factor: None,
            active: Some(account.active),
            status: account.status.clone(),
        })
    }

    /// Get session info
    pub async fn get_session(&self, access_token: &str) -> Result<Session> {
        // Validate access token
        let claims = self.token_manager.validate_token(access_token, "access")?;

        // Get account
        let account = {
            let accounts = self.accounts_by_did.read().unwrap();
            accounts
                .get(&claims.sub)
                .cloned()
                .ok_or(AuthError::AccountNotFound)?
        };

        Ok(Session {
            access_jwt: String::new(), // Don't return tokens in getSession
            refresh_jwt: String::new(),
            handle: account.handle.clone(),
            did: account.did.to_string(),
            did_doc: None,
            email: account.email.clone(),
            email_confirmed: Some(account.email_confirmed),
            email_auth_factor: Some(account.email_auth_factor),
            active: Some(account.active),
            status: account.status.clone(),
        })
    }

    /// Delete session (logout)
    pub async fn delete_session(&self, refresh_token: &str) -> Result<()> {
        // Validate refresh token
        let _claims = self.token_manager.validate_token(refresh_token, "refresh")?;

        // Delete session (in production, invalidate all sessions for this user)
        self.session_store.delete(refresh_token);

        Ok(())
    }

    /// Find account by identifier (handle, email, or DID)
    fn find_account_by_identifier(&self, identifier: &str) -> Result<Account> {
        let accounts_by_did = self.accounts_by_did.read().unwrap();
        let accounts_by_handle = self.accounts_by_handle.read().unwrap();

        // Try DID first
        if identifier.starts_with("did:") {
            return accounts_by_did
                .get(identifier)
                .cloned()
                .ok_or(AuthError::AccountNotFound);
        }

        // Try handle
        if let Some(did) = accounts_by_handle.get(identifier) {
            return accounts_by_did
                .get(did)
                .cloned()
                .ok_or(AuthError::AccountNotFound);
        }

        // Try email
        for account in accounts_by_did.values() {
            if let Some(ref email) = account.email {
                if email == identifier {
                    return Ok(account.clone());
                }
            }
        }

        Err(AuthError::AccountNotFound)
    }

    /// Add an invite code
    pub fn add_invite_code(&mut self, code: String, available: usize) {
        let mut codes = self.invite_codes.write().unwrap();
        codes.insert(
            code.clone(),
            InviteCode {
                code,
                available,
                disabled: false,
                created_at: Utc::now(),
            },
        );
    }

    /// Validate and use an invite code
    fn validate_and_use_invite_code(&self, code: &str) -> Result<()> {
        let mut codes = self.invite_codes.write().unwrap();

        let invite = codes
            .get_mut(code)
            .ok_or(AuthError::InvalidInviteCode)?;

        if invite.disabled || invite.available == 0 {
            return Err(AuthError::InvalidInviteCode);
        }

        invite.available -= 1;
        Ok(())
    }
}

#[cfg(test)]
impl Default for AuthManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_hasher() {
        let password = "my_secure_password_123";
        let hash = PasswordHasher::hash(password).unwrap();

        // Verify correct password
        assert!(PasswordHasher::verify(password, &hash).unwrap());

        // Verify wrong password fails
        assert!(!PasswordHasher::verify("wrong_password", &hash).unwrap());
    }

    #[test]
    fn test_token_manager() {
        let manager = TokenManager::new(
            "https://test.pds.com".to_string(),
            "atproto".to_string(),
        );

        let did = "did:plc:test123";

        // Generate access token
        let access_token = manager.generate_access_token(did).unwrap();
        let claims = manager.validate_token(&access_token, "access").unwrap();

        assert_eq!(claims.sub, did);
        assert_eq!(claims.token_type, "access");

        // Generate refresh token
        let refresh_token = manager.generate_refresh_token(did).unwrap();
        let claims = manager.validate_token(&refresh_token, "refresh").unwrap();

        assert_eq!(claims.sub, did);
        assert_eq!(claims.token_type, "refresh");
    }

    #[test]
    fn test_session_store() {
        let store = SessionStore::new();
        let token = "test_token";
        let did = "did:plc:test123";
        let expiry = Utc::now() + Duration::hours(1);

        // Store session
        store.store(token, did, expiry);

        // Get session
        assert_eq!(store.get(token), Some(did.to_string()));

        // Delete session
        store.delete(token);
        assert_eq!(store.get(token), None);
    }

    #[tokio::test]
    async fn test_create_account() {
        let mut auth = AuthManager::new();

        let req = AccountCreate {
            handle: "alice.test.com".to_string(),
            email: Some("alice@example.com".to_string()),
            password: Some("secure_password_123".to_string()),
            invite_code: None,
            did: None,
            verification_code: None,
            verification_phone: None,
            recovery_key: None,
            plc_op: None,
        };

        let result = auth.create_account(req).await.unwrap();

        assert_eq!(result.handle, "alice.test.com");
        assert!(!result.access_jwt.is_empty());
        assert!(!result.refresh_jwt.is_empty());
        assert!(result.did.starts_with("did:plc:"));
    }

    #[tokio::test]
    async fn test_create_session() {
        let mut auth = AuthManager::new();

        // Create account
        let req = AccountCreate {
            handle: "bob.test.com".to_string(),
            email: Some("bob@example.com".to_string()),
            password: Some("my_password_456".to_string()),
            invite_code: None,
            did: None,
            verification_code: None,
            verification_phone: None,
            recovery_key: None,
            plc_op: None,
        };

        auth.create_account(req).await.unwrap();

        // Create session (login)
        let session = auth
            .create_session("bob.test.com", "my_password_456", None)
            .await
            .unwrap();

        assert_eq!(session.handle, "bob.test.com");
        assert!(!session.access_jwt.is_empty());
        assert!(!session.refresh_jwt.is_empty());
    }

    #[tokio::test]
    async fn test_invalid_credentials() {
        let mut auth = AuthManager::new();

        // Create account
        let req = AccountCreate {
            handle: "charlie.test.com".to_string(),
            email: None,
            password: Some("correct_password".to_string()),
            invite_code: None,
            did: None,
            verification_code: None,
            verification_phone: None,
            recovery_key: None,
            plc_op: None,
        };

        auth.create_account(req).await.unwrap();

        // Try wrong password
        let result = auth
            .create_session("charlie.test.com", "wrong_password", None)
            .await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
    }

    #[tokio::test]
    async fn test_refresh_session() {
        let mut auth = AuthManager::new();

        // Create account
        let req = AccountCreate {
            handle: "dave.test.com".to_string(),
            email: None,
            password: Some("password123".to_string()),
            invite_code: None,
            did: None,
            verification_code: None,
            verification_phone: None,
            recovery_key: None,
            plc_op: None,
        };

        let created = auth.create_account(req).await.unwrap();

        // Refresh session
        let refreshed = auth.refresh_session(&created.refresh_jwt).await.unwrap();

        assert_eq!(refreshed.handle, "dave.test.com");
        assert!(!refreshed.access_jwt.is_empty());
        assert!(!refreshed.refresh_jwt.is_empty());
    }

    #[tokio::test]
    async fn test_invite_code() {
        let mut auth = AuthManager::new();
        auth.require_invite_code = true;

        // Add invite code
        auth.add_invite_code("TEST-CODE-123".to_string(), 1);

        // Create account with invite code
        let req = AccountCreate {
            handle: "eve.test.com".to_string(),
            email: None,
            password: Some("password123".to_string()),
            invite_code: Some("TEST-CODE-123".to_string()),
            did: None,
            verification_code: None,
            verification_phone: None,
            recovery_key: None,
            plc_op: None,
        };

        let result = auth.create_account(req).await;
        assert!(result.is_ok());

        // Try to create another account with same code (should fail - used up)
        let req2 = AccountCreate {
            handle: "frank.test.com".to_string(),
            email: None,
            password: Some("password123".to_string()),
            invite_code: Some("TEST-CODE-123".to_string()),
            did: None,
            verification_code: None,
            verification_phone: None,
            recovery_key: None,
            plc_op: None,
        };

        let result2 = auth.create_account(req2).await;
        assert!(matches!(result2, Err(AuthError::InvalidInviteCode)));
    }

    #[tokio::test]
    async fn test_handle_not_available() {
        let mut auth = AuthManager::new();

        // Create first account
        let req1 = AccountCreate {
            handle: "same.test.com".to_string(),
            email: None,
            password: Some("password123".to_string()),
            invite_code: None,
            did: None,
            verification_code: None,
            verification_phone: None,
            recovery_key: None,
            plc_op: None,
        };

        auth.create_account(req1).await.unwrap();

        // Try to create second account with same handle
        let req2 = AccountCreate {
            handle: "same.test.com".to_string(),
            email: None,
            password: Some("different_password".to_string()),
            invite_code: None,
            did: None,
            verification_code: None,
            verification_phone: None,
            recovery_key: None,
            plc_op: None,
        };

        let result = auth.create_account(req2).await;
        assert!(matches!(result, Err(AuthError::HandleNotAvailable(_))));
    }
}
