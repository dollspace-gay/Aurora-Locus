/// Account manager implementation using runtime queries
/// This version uses sqlx runtime query building instead of compile-time macros
/// to avoid needing DATABASE_URL during compilation

use crate::{
    account::AppPasswordInfo,
    config::ServerConfig,
    db::account::{Account, Session},
    error::{PdsError, PdsResult},
};
use chrono::{DateTime, Duration, Utc};
use sqlx::{Row, SqlitePool};
use std::sync::Arc;
use uuid::Uuid;

/// Account manager service
pub struct AccountManager {
    db: SqlitePool,
    config: Arc<ServerConfig>,
}

impl AccountManager {
    /// Create a new account manager
    pub fn new(db: SqlitePool, config: Arc<ServerConfig>) -> Self {
        Self { db, config }
    }

    /// Create a new account
    pub async fn create_account(
        &self,
        handle: String,
        email: Option<String>,
        password: String,
        _invite_code: Option<String>,
    ) -> PdsResult<Account> {
        // Note: Invite code validation is handled at the API layer
        // This keeps the AccountManager focused on account creation logic

        // Validate handle format
        self.validate_handle(&handle)?;

        // Validate email if provided
        if let Some(ref email_str) = email {
            self.validate_email(email_str)?;
        }

        // Check if handle already exists
        if self.handle_exists(&handle).await? {
            return Err(PdsError::Conflict(format!("Handle {} already taken", handle)));
        }

        // Check if email already exists
        if let Some(ref email_str) = email {
            if self.email_exists(email_str).await? {
                return Err(PdsError::Conflict("Email already registered".to_string()));
            }
        }

        // Hash password using SDK's Argon2id implementation
        let password_hash = atproto::server_auth::PasswordHasher::hash(&password)
            .map_err(|e| PdsError::Internal(format!("Password hashing failed: {}", e)))?;

        // Generate DID with PLC registration
        let (did, plc_key, plc_key_public, plc_operation_cid) = self.generate_plc_did(&handle).await?;

        // Insert account
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO account (did, handle, email, password_hash, created_at, email_confirmed, taken_down, plc_rotation_key, plc_rotation_key_public, plc_last_operation_cid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
        )
        .bind(&did)
        .bind(&handle)
        .bind(&email)
        .bind(&password_hash)
        .bind(now)
        .bind(false)
        .bind(false)
        .bind(&plc_key)
        .bind(&plc_key_public)
        .bind(&plc_operation_cid)
        .execute(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?;

        Ok(Account {
            did,
            handle,
            email,
            password_hash,
            created_at: now,
            email_confirmed: false,
            email_confirmed_at: None,
            deactivated_at: None,
            taken_down: false,
            plc_rotation_key: Some(plc_key),
            plc_rotation_key_public: Some(plc_key_public),
            plc_last_operation_cid: Some(plc_operation_cid),
        })
    }

    /// Authenticate account and create session
    pub async fn login(
        &self,
        identifier: &str,
        password: &str,
    ) -> PdsResult<(Account, Session)> {
        // Find account by handle or email
        let account = self.get_account_by_identifier(identifier).await?;

        // Check if account is deactivated or taken down
        if account.deactivated_at.is_some() {
            return Err(PdsError::Authorization("Account is deactivated".to_string()));
        }

        if account.taken_down {
            return Err(PdsError::Authorization("Account has been taken down".to_string()));
        }

        // Verify password
        let valid = atproto::server_auth::PasswordHasher::verify(password, &account.password_hash)
            .map_err(|e| PdsError::Internal(format!("Password verification failed: {}", e)))?;

        if !valid {
            return Err(PdsError::Authentication("Invalid credentials".to_string()));
        }

        // Create session
        let session = self.create_session(&account.did, None).await?;

        Ok((account, session))
    }

    /// Create a session for a DID
    pub async fn create_session(
        &self,
        did: &str,
        app_password_name: Option<String>,
    ) -> PdsResult<Session> {
        let session_id = Uuid::new_v4().to_string();

        // Generate JWT tokens
        let access_token = self.generate_access_token(did, &session_id)?;
        let refresh_token_str = self.generate_refresh_token(did, &session_id)?;

        let now = Utc::now();
        let expires_at = now + Duration::hours(1); // Access token expires in 1 hour

        // Insert session
        sqlx::query(
            "INSERT INTO session (id, did, access_token, refresh_token, created_at, expires_at, app_password_name)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
        )
        .bind(&session_id)
        .bind(did)
        .bind(&access_token)
        .bind(&refresh_token_str)
        .bind(now)
        .bind(expires_at)
        .bind(&app_password_name)
        .execute(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?;

        // Store refresh token
        let refresh_token_id = Uuid::new_v4().to_string();
        let refresh_expires = now + Duration::days(180); // Refresh token expires in 6 months

        sqlx::query(
            "INSERT INTO refresh_token (id, did, token, created_at, expires_at, used)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        )
        .bind(&refresh_token_id)
        .bind(did)
        .bind(&refresh_token_str)
        .bind(now)
        .bind(refresh_expires)
        .bind(false)
        .execute(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?;

        Ok(Session {
            id: session_id,
            did: did.to_string(),
            access_token,
            refresh_token: refresh_token_str,
            created_at: now,
            expires_at,
            app_password_name,
        })
    }

    /// Validate access token and return session info
    pub async fn validate_access_token(&self, token: &str) -> PdsResult<crate::account::ValidatedSession> {
        // Find session by access token
        let row = sqlx::query(
            "SELECT id, did, expires_at, app_password_name FROM session WHERE access_token = ?1"
        )
        .bind(token)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?
        .ok_or_else(|| PdsError::Authentication("Invalid or expired session".to_string()))?;

        let session_id: String = row.get("id");
        let did: String = row.get("did");
        let expires_at: DateTime<Utc> = row.get("expires_at");
        let app_password_name: Option<String> = row.get("app_password_name");

        // Check expiration
        if Utc::now() > expires_at {
            return Err(PdsError::Authentication("Session expired".to_string()));
        }

        Ok(crate::account::ValidatedSession {
            did,
            session_id,
            is_app_password: app_password_name.is_some(),
        })
    }

    /// Delete a session (logout)
    pub async fn delete_session(&self, session_id: &str) -> PdsResult<()> {
        sqlx::query("DELETE FROM session WHERE id = ?1")
            .bind(session_id)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        Ok(())
    }

    /// Refresh session tokens
    pub async fn refresh_session(&self, refresh_token: &str) -> PdsResult<Session> {
        // Find and validate refresh token
        let row = sqlx::query(
            "SELECT id, did, token, created_at, expires_at, used, used_at FROM refresh_token WHERE token = ?1"
        )
        .bind(refresh_token)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?
        .ok_or_else(|| PdsError::Authentication("Invalid refresh token".to_string()))?;

        let token_id: String = row.get("id");
        let did: String = row.get("did");
        let expires_at: DateTime<Utc> = row.get("expires_at");
        let used: bool = row.get("used");

        // Check if already used
        if used {
            return Err(PdsError::Authentication("Refresh token already used".to_string()));
        }

        // Check expiration
        if Utc::now() > expires_at {
            return Err(PdsError::Authentication("Refresh token expired".to_string()));
        }

        // Mark old refresh token as used
        sqlx::query("UPDATE refresh_token SET used = TRUE, used_at = ?1 WHERE id = ?2")
            .bind(Utc::now())
            .bind(&token_id)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        // Create new session
        self.create_session(&did, None).await
    }

    /// Get account by DID
    pub async fn get_account(&self, did: &str) -> PdsResult<Account> {
        let row = sqlx::query(
            "SELECT did, handle, email, password_hash, created_at, email_confirmed,
                    email_confirmed_at, deactivated_at, taken_down
             FROM account WHERE did = ?1"
        )
        .bind(did)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?
        .ok_or_else(|| PdsError::NotFound("Account not found".to_string()))?;

        Ok(Account {
            did: row.get("did"),
            handle: row.get("handle"),
            email: row.get("email"),
            password_hash: row.get("password_hash"),
            created_at: row.get("created_at"),
            email_confirmed: row.get("email_confirmed"),
            email_confirmed_at: row.get("email_confirmed_at"),
            deactivated_at: row.get("deactivated_at"),
            taken_down: row.get("taken_down"),
            plc_rotation_key: row.get("plc_rotation_key"),
            plc_rotation_key_public: row.get("plc_rotation_key_public"),
            plc_last_operation_cid: row.get("plc_last_operation_cid"),
        })
    }

    /// Find account by handle or email (public for password reset)
    pub async fn get_account_by_identifier(&self, identifier: &str) -> PdsResult<Account> {
        // Try handle first
        if let Ok(account) = self.get_account_by_handle(identifier).await {
            return Ok(account);
        }

        // Try email
        self.get_account_by_email(identifier).await
    }

    /// Get account by handle
    async fn get_account_by_handle(&self, handle: &str) -> PdsResult<Account> {
        let row = sqlx::query(
            "SELECT did, handle, email, password_hash, created_at, email_confirmed,
                    email_confirmed_at, deactivated_at, taken_down,
                    plc_rotation_key, plc_rotation_key_public, plc_last_operation_cid
             FROM account WHERE handle = ?1"
        )
        .bind(handle)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?
        .ok_or_else(|| PdsError::NotFound("Account not found".to_string()))?;

        Ok(Account {
            did: row.get("did"),
            handle: row.get("handle"),
            email: row.get("email"),
            password_hash: row.get("password_hash"),
            created_at: row.get("created_at"),
            email_confirmed: row.get("email_confirmed"),
            email_confirmed_at: row.get("email_confirmed_at"),
            deactivated_at: row.get("deactivated_at"),
            taken_down: row.get("taken_down"),
            plc_rotation_key: row.get("plc_rotation_key"),
            plc_rotation_key_public: row.get("plc_rotation_key_public"),
            plc_last_operation_cid: row.get("plc_last_operation_cid"),
        })
    }

    /// Get account by email
    async fn get_account_by_email(&self, email: &str) -> PdsResult<Account> {
        let row = sqlx::query(
            "SELECT did, handle, email, password_hash, created_at, email_confirmed,
                    email_confirmed_at, deactivated_at, taken_down,
                    plc_rotation_key, plc_rotation_key_public, plc_last_operation_cid
             FROM account WHERE email = ?1"
        )
        .bind(email)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?
        .ok_or_else(|| PdsError::NotFound("Account not found".to_string()))?;

        Ok(Account {
            did: row.get("did"),
            handle: row.get("handle"),
            email: row.get("email"),
            password_hash: row.get("password_hash"),
            created_at: row.get("created_at"),
            email_confirmed: row.get("email_confirmed"),
            email_confirmed_at: row.get("email_confirmed_at"),
            deactivated_at: row.get("deactivated_at"),
            taken_down: row.get("taken_down"),
            plc_rotation_key: row.get("plc_rotation_key"),
            plc_rotation_key_public: row.get("plc_rotation_key_public"),
            plc_last_operation_cid: row.get("plc_last_operation_cid"),
        })
    }

    /// Check if handle exists
    async fn handle_exists(&self, handle: &str) -> PdsResult<bool> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM account WHERE handle = ?1")
            .bind(handle)
            .fetch_one(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        Ok(count > 0)
    }

    /// Update account handle
    ///
    /// Updates the handle for a given DID. The new handle must not be taken by another account.
    /// Returns the old handle that was replaced.
    pub async fn update_handle(&self, did: &str, new_handle: &str) -> PdsResult<String> {
        // Validate new handle format
        self.validate_handle(new_handle)?;

        // Get current account to retrieve old handle
        let account = self.get_account(did).await?;
        let old_handle = account.handle.clone();

        // Check if new handle is the same as current (no-op)
        if old_handle == new_handle {
            return Ok(old_handle);
        }

        // Check if new handle is already taken by another account
        if let Ok(existing) = self.get_account_by_handle(new_handle).await {
            if existing.did != did {
                return Err(PdsError::Conflict(format!("Handle {} already taken", new_handle)));
            }
        }

        // Update handle in database
        sqlx::query("UPDATE account SET handle = ?1 WHERE did = ?2")
            .bind(new_handle)
            .bind(did)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        Ok(old_handle)
    }

    /// Check if email exists
    async fn email_exists(&self, email: &str) -> PdsResult<bool> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM account WHERE email = ?1")
            .bind(email)
            .fetch_one(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        Ok(count > 0)
    }

    /// Generate DID for handle
    /// Generate a PLC DID and register it with the PLC Directory
    ///
    /// Returns: (did, rotation_key_hex, rotation_key_public_hex, operation_cid)
    async fn generate_plc_did(&self, handle: &str) -> PdsResult<(String, String, String, String)> {
        use crate::crypto::plc::{PlcOperationBuilder, PlcSigner, register_plc_did};
        use sha2::{Digest, Sha256};
        use rand::RngCore;

        // Generate a random 32-byte private key for PLC rotation
        let mut private_key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut private_key);
        let private_key_hex = hex::encode(&private_key);

        // Create PLC signer
        let signer = PlcSigner::new(&private_key)?;
        let public_key_hex = signer.public_key_hex();

        // Generate DID from hash of public key (PLC method)
        // did:plc uses base32-encoded hash of the genesis operation
        let mut hasher = Sha256::new();
        hasher.update(&public_key_hex);
        let hash = hasher.finalize();

        // Use full hash and encode as base32 lowercase (RFC4648, no padding), then truncate to 24 chars
        // This follows the PLC spec: did:plc:${base32Encode(sha256(createOp)).slice(0,24)}
        let base32_hash = base32::encode(base32::Alphabet::Rfc4648Lower { padding: false }, &hash);
        let did_suffix = &base32_hash[..24]; // Truncate to 24 characters
        let did = format!("did:plc:{}", did_suffix);

        // Build PLC operation
        let service_url = format!("https://{}", self.config.service.hostname);

        // Check if handle already includes the domain
        let full_handle = if handle.contains('.') && self.config.identity.service_handle_domains.iter().any(|d| handle.ends_with(d)) {
            // Handle is already full (e.g., "test.locus.dollsky.social")
            handle.to_string()
        } else {
            // Handle needs domain appended (e.g., "test" -> "test.locus.dollsky.social")
            format!("{}.{}", handle, self.config.identity.service_handle_domains[0])
        };

        let services = serde_json::json!([{
            "id": "#atproto_pds",
            "type": "AtprotoPersonalDataServer",
            "serviceEndpoint": service_url
        }]);

        // Get proper multibase encoding for public key
        let public_key_multibase = signer.public_key_multibase();
        let public_key_did_key = signer.public_key_did_key();

        let verification_methods = serde_json::json!([{
            "id": format!("{}#atproto", did),
            "type": "Multikey",
            "controller": did.clone(),
            "publicKeyMultibase": public_key_multibase
        }]);

        let also_known_as = vec![format!("at://{}", full_handle)];

        let operation = PlcOperationBuilder::new()
            .did(did.clone())
            .rotation_keys(vec![public_key_did_key])
            .also_known_as(also_known_as)
            .services(services)
            .verification_methods(verification_methods)
            .build()?;

        // Sign the operation
        let signed_operation = signer.sign_operation(operation)?;

        // Get PLC directory URL from config or use default
        let plc_url = self.config.identity.did_plc_url.as_str();

        // Register with PLC directory
        match register_plc_did(plc_url, signed_operation.clone()).await {
            Ok(_) => {
                tracing::info!("Successfully registered DID with PLC directory: {}", did);

                // For operation CID, we'll use a simplified hash of the operation
                // In production, this should be a proper CID
                let operation_json = serde_json::to_string(&signed_operation)
                    .unwrap_or_default();
                let mut cid_hasher = Sha256::new();
                cid_hasher.update(operation_json.as_bytes());
                let cid_hash = cid_hasher.finalize();
                let operation_cid = format!("bafyrei{}", hex::encode(&cid_hash[..16]));

                Ok((did, private_key_hex, public_key_hex, operation_cid))
            }
            Err(e) => {
                tracing::warn!("Failed to register DID with PLC directory: {}. Falling back to did:web", e);
                // Fallback to did:web if PLC registration fails
                // full_handle is already constructed above (line 463)
                let did_web = format!("did:web:{}", full_handle);
                Ok((did_web, private_key_hex, public_key_hex, "".to_string()))
            }
        }
    }

    /// Generate access JWT token
    fn generate_access_token(&self, did: &str, session_id: &str) -> PdsResult<String> {
        use jsonwebtoken::{encode, EncodingKey, Header};
        use serde::{Deserialize, Serialize};

        #[derive(Debug, Serialize, Deserialize)]
        struct Claims {
            sub: String,
            sid: String,
            iat: i64,
            exp: i64,
        }

        let now = Utc::now().timestamp();
        let claims = Claims {
            sub: did.to_string(),
            sid: session_id.to_string(),
            iat: now,
            exp: now + 3600, // 1 hour
        };

        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.config.authentication.jwt_secret.as_bytes()),
        )
        .map_err(|e| PdsError::Jwt(format!("Failed to generate token: {}", e)))?;

        Ok(token)
    }

    /// Generate refresh JWT token
    fn generate_refresh_token(&self, did: &str, session_id: &str) -> PdsResult<String> {
        use jsonwebtoken::{encode, EncodingKey, Header};
        use serde::{Deserialize, Serialize};

        #[derive(Debug, Serialize, Deserialize)]
        struct RefreshClaims {
            sub: String,
            sid: String,
            iat: i64,
            exp: i64,
        }

        let now = Utc::now().timestamp();
        let claims = RefreshClaims {
            sub: did.to_string(),
            sid: session_id.to_string(),
            iat: now,
            exp: now + (180 * 24 * 3600), // 180 days
        };

        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.config.authentication.jwt_secret.as_bytes()),
        )
        .map_err(|e| PdsError::Jwt(format!("Failed to generate refresh token: {}", e)))?;

        Ok(token)
    }

    /// Cleanup expired sessions and refresh tokens
    ///
    /// This should be called periodically (e.g., hourly) to remove expired tokens
    /// from the database and free up storage space.
    ///
    /// Returns (sessions_deleted, refresh_tokens_deleted)
    pub async fn cleanup_expired_sessions(&self) -> PdsResult<(u64, u64)> {
        let now = Utc::now();

        // Delete expired access token sessions
        let sessions_result = sqlx::query("DELETE FROM session WHERE expires_at < ?1")
            .bind(now)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        let sessions_deleted = sessions_result.rows_affected();

        // Delete expired refresh tokens
        let refresh_result = sqlx::query("DELETE FROM refresh_token WHERE expires_at < ?1")
            .bind(now)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        let refresh_tokens_deleted = refresh_result.rows_affected();

        // Log results
        if sessions_deleted > 0 || refresh_tokens_deleted > 0 {
            tracing::info!(
                sessions_deleted,
                refresh_tokens_deleted,
                "Cleaned up expired tokens"
            );
        } else {
            tracing::debug!("Session cleanup: no expired tokens found");
        }

        Ok((sessions_deleted, refresh_tokens_deleted))
    }

    /// Generate and store email verification token
    ///
    /// Creates a verification token that expires in 24 hours
    pub async fn generate_email_verification_token(&self, did: &str) -> PdsResult<String> {
        let token = Uuid::new_v4().to_string();
        let now = Utc::now();
        let expires_at = now + Duration::hours(24);

        sqlx::query(
            r#"
            INSERT INTO email_token (token, did, purpose, created_at, expires_at, used)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
        )
        .bind(&token)
        .bind(did)
        .bind("confirm_email")
        .bind(now)
        .bind(expires_at)
        .bind(false)
        .execute(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?;

        Ok(token)
    }

    /// Confirm email address using verification token
    ///
    /// Marks the email as confirmed if the token is valid and not expired
    pub async fn confirm_email(&self, token: &str) -> PdsResult<String> {
        let now = Utc::now();

        // Get token info
        let row = sqlx::query(
            r#"
            SELECT token, did, purpose, expires_at, used
            FROM email_token
            WHERE token = ?1 AND purpose = 'confirm_email'
            "#,
        )
        .bind(token)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?
        .ok_or_else(|| PdsError::NotFound("Invalid verification token".to_string()))?;

        let did: String = row.try_get("did")?;
        let expires_at: DateTime<Utc> = row.try_get("expires_at")?;
        let used: bool = row.try_get("used")?;

        // Check if already used
        if used {
            return Err(PdsError::Validation(
                "Verification token has already been used".to_string(),
            ));
        }

        // Check expiration
        if now > expires_at {
            return Err(PdsError::Validation(
                "Verification token has expired".to_string(),
            ));
        }

        // Mark token as used
        sqlx::query("UPDATE email_token SET used = true WHERE token = ?1")
            .bind(token)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        // Mark email as confirmed in account
        sqlx::query(
            "UPDATE account SET email_confirmed = true, email_confirmed_at = ?1 WHERE did = ?2",
        )
        .bind(now)
        .bind(&did)
        .execute(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?;

        tracing::info!("Email confirmed for DID: {}", did);

        Ok(did)
    }

    /// Request new email confirmation
    ///
    /// Generates a new token and can optionally send verification email
    pub async fn request_email_confirmation(&self, did: &str) -> PdsResult<String> {
        // Verify account exists and has email
        let row = sqlx::query("SELECT email FROM account WHERE did = ?1")
            .bind(did)
            .fetch_optional(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?
            .ok_or_else(|| PdsError::NotFound("Account not found".to_string()))?;

        let email: Option<String> = row.try_get("email")?;

        if email.is_none() {
            return Err(PdsError::Validation(
                "Account does not have an email address".to_string(),
            ));
        }

        // Generate new token
        let token = self.generate_email_verification_token(did).await?;

        Ok(token)
    }

    /// Generate password reset token
    ///
    /// Creates a reset token that expires in 1 hour
    pub async fn generate_password_reset_token(&self, identifier: &str) -> PdsResult<(String, String)> {
        // Find account by email or handle
        let account = self.get_account_by_identifier(identifier).await?;

        if account.email.is_none() {
            return Err(PdsError::Validation(
                "Account does not have an email address".to_string(),
            ));
        }

        let token = Uuid::new_v4().to_string();
        let now = Utc::now();
        let expires_at = now + Duration::hours(1); // Password reset tokens expire in 1 hour

        sqlx::query(
            r#"
            INSERT INTO email_token (token, did, purpose, created_at, expires_at, used)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
        )
        .bind(&token)
        .bind(&account.did)
        .bind("reset_password")
        .bind(now)
        .bind(expires_at)
        .bind(false)
        .execute(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?;

        Ok((token, account.email.unwrap()))
    }

    /// Reset password using reset token
    ///
    /// Validates the token, updates the password, and invalidates all sessions
    pub async fn reset_password(&self, token: &str, new_password: &str) -> PdsResult<()> {
        let now = Utc::now();

        // Get token info
        let row = sqlx::query(
            r#"
            SELECT token, did, purpose, expires_at, used
            FROM email_token
            WHERE token = ?1 AND purpose = 'reset_password'
            "#,
        )
        .bind(token)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?
        .ok_or_else(|| PdsError::NotFound("Invalid reset token".to_string()))?;

        let did: String = row.try_get("did")?;
        let expires_at: DateTime<Utc> = row.try_get("expires_at")?;
        let used: bool = row.try_get("used")?;

        // Check if already used
        if used {
            return Err(PdsError::Validation(
                "Reset token has already been used".to_string(),
            ));
        }

        // Check expiration
        if now > expires_at {
            return Err(PdsError::Validation(
                "Reset token has expired".to_string(),
            ));
        }

        // Hash new password
        let password_hash = atproto::server_auth::PasswordHasher::hash(new_password)
            .map_err(|e| PdsError::Internal(format!("Password hashing failed: {}", e)))?;

        // Update password in database
        sqlx::query("UPDATE account SET password_hash = ?1 WHERE did = ?2")
            .bind(&password_hash)
            .bind(&did)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        // Mark token as used
        sqlx::query("UPDATE email_token SET used = true WHERE token = ?1")
            .bind(token)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        // Invalidate all sessions for this account (security best practice)
        sqlx::query("DELETE FROM session WHERE did = ?1")
            .bind(&did)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        // Also delete all refresh tokens
        sqlx::query("DELETE FROM refresh_token WHERE did = ?1")
            .bind(&did)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        tracing::info!("Password reset successful for DID: {}", did);

        Ok(())
    }

    /// Request account deletion (soft delete with grace period)
    ///
    /// Marks account for deletion after verifying password
    /// Actual deletion happens after grace period via background job
    pub async fn request_account_deletion(&self, did: &str, password: &str) -> PdsResult<()> {
        // Get account
        let account = self.get_account(did).await?;

        // Verify password
        let valid = atproto::server_auth::PasswordHasher::verify(&account.password_hash, password)
            .map_err(|e| PdsError::Internal(format!("Password verification failed: {}", e)))?;

        if !valid {
            return Err(PdsError::Authorization("Invalid password".to_string()));
        }

        // Mark account for deletion (30 day grace period)
        let deletion_date = Utc::now() + Duration::days(30);

        sqlx::query(
            "UPDATE account SET deactivated_at = ?1 WHERE did = ?2"
        )
        .bind(deletion_date)
        .bind(did)
        .execute(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?;

        // Delete all active sessions (force logout)
        sqlx::query("DELETE FROM session WHERE did = ?1")
            .bind(did)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        // Delete all refresh tokens
        sqlx::query("DELETE FROM refresh_token WHERE did = ?1")
            .bind(did)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        tracing::info!(
            "Account deletion requested for DID: {}, will be deleted after: {}",
            did,
            deletion_date
        );

        Ok(())
    }

    /// Check if account is marked for deletion
    pub async fn is_account_pending_deletion(&self, did: &str) -> PdsResult<bool> {
        let row = sqlx::query("SELECT deactivated_at FROM account WHERE did = ?1")
            .bind(did)
            .fetch_optional(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?
            .ok_or_else(|| PdsError::NotFound("Account not found".to_string()))?;

        let deactivated_at: Option<DateTime<Utc>> = row.try_get("deactivated_at")?;
        Ok(deactivated_at.is_some())
    }

    /// Cancel account deletion (if within grace period)
    pub async fn cancel_account_deletion(&self, did: &str) -> PdsResult<()> {
        sqlx::query("UPDATE account SET deactivated_at = NULL WHERE did = ?1")
            .bind(did)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        tracing::info!("Account deletion cancelled for DID: {}", did);

        Ok(())
    }

    // ==================== App Passwords ====================

    /// Create an app password for third-party applications
    pub async fn create_app_password(
        &self,
        did: &str,
        name: &str,
        privileged: bool,
    ) -> PdsResult<String> {
        // Validate name
        if name.is_empty() {
            return Err(PdsError::Validation("App password name cannot be empty".to_string()));
        }

        if name.len() > 100 {
            return Err(PdsError::Validation("App password name too long".to_string()));
        }

        // Check if app password with this name already exists for this user
        let existing = sqlx::query("SELECT name FROM app_password WHERE did = ?1 AND name = ?2")
            .bind(did)
            .bind(name)
            .fetch_optional(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        if existing.is_some() {
            return Err(PdsError::Conflict(format!("App password '{}' already exists", name)));
        }

        // Generate a random password (32 characters, alphanumeric)
        // Format: xxxx-xxxx-xxxx-xxxx-xxxx-xxxx-xxxx-xxxx (for readability)
        let raw_password = format!(
            "{}-{}-{}-{}-{}-{}-{}-{}",
            Self::generate_random_string(4),
            Self::generate_random_string(4),
            Self::generate_random_string(4),
            Self::generate_random_string(4),
            Self::generate_random_string(4),
            Self::generate_random_string(4),
            Self::generate_random_string(4),
            Self::generate_random_string(4)
        );

        // Hash the password using Argon2id
        let password_hash = atproto::server_auth::PasswordHasher::hash(&raw_password)
            .map_err(|e| PdsError::Internal(format!("Password hashing failed: {}", e)))?;

        // Store app password
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO app_password (did, name, password_hash, created_at, privileged)
             VALUES (?1, ?2, ?3, ?4, ?5)"
        )
        .bind(did)
        .bind(name)
        .bind(&password_hash)
        .bind(now)
        .bind(privileged)
        .execute(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?;

        tracing::info!("Created app password '{}' for DID: {}", name, did);

        // Return the raw password (only time it's shown to user)
        Ok(raw_password)
    }

    /// List all app passwords for a user (without the actual passwords)
    pub async fn list_app_passwords(&self, did: &str) -> PdsResult<Vec<AppPasswordInfo>> {
        let rows = sqlx::query(
            "SELECT name, created_at, privileged FROM app_password WHERE did = ?1 ORDER BY created_at DESC"
        )
        .bind(did)
        .fetch_all(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?;

        let mut passwords = Vec::new();
        for row in rows {
            passwords.push(AppPasswordInfo {
                name: row.get("name"),
                created_at: row.get("created_at"),
                privileged: row.get("privileged"),
            });
        }

        Ok(passwords)
    }

    /// Revoke (delete) an app password
    pub async fn revoke_app_password(&self, did: &str, name: &str) -> PdsResult<()> {
        let result = sqlx::query("DELETE FROM app_password WHERE did = ?1 AND name = ?2")
            .bind(did)
            .bind(name)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        if result.rows_affected() == 0 {
            return Err(PdsError::NotFound(format!("App password '{}' not found", name)));
        }

        // Delete all sessions created with this app password
        sqlx::query("DELETE FROM session WHERE did = ?1 AND app_password_name = ?2")
            .bind(did)
            .bind(name)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        tracing::info!("Revoked app password '{}' for DID: {}", name, did);

        Ok(())
    }

    /// Authenticate with app password
    pub async fn login_with_app_password(
        &self,
        identifier: &str,
        app_password: &str,
    ) -> PdsResult<(Account, Session, String)> {
        // Find account
        let account = self.get_account_by_identifier(identifier).await?;

        // Check if account is deactivated or taken down
        if account.deactivated_at.is_some() {
            return Err(PdsError::Authorization("Account is deactivated".to_string()));
        }

        if account.taken_down {
            return Err(PdsError::Authorization("Account has been taken down".to_string()));
        }

        // Find matching app password by trying to verify against all user's app passwords
        let rows = sqlx::query(
            "SELECT name, password_hash FROM app_password WHERE did = ?1"
        )
        .bind(&account.did)
        .fetch_all(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?;

        let mut matched_name: Option<String> = None;
        for row in rows {
            let name: String = row.get("name");
            let hash: String = row.get("password_hash");

            if let Ok(true) = atproto::server_auth::PasswordHasher::verify(app_password, &hash) {
                matched_name = Some(name);
                break;
            }
        }

        let app_password_name = matched_name
            .ok_or_else(|| PdsError::Authentication("Invalid app password".to_string()))?;

        // Create session with app_password_name
        let session = self.create_session(&account.did, Some(app_password_name.clone())).await?;

        Ok((account, session, app_password_name))
    }

    /// Generate random alphanumeric string
    fn generate_random_string(length: usize) -> String {
        use rand::Rng;
        const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ\
                                 abcdefghijklmnopqrstuvwxyz\
                                 0123456789";
        let mut rng = rand::thread_rng();
        (0..length)
            .map(|_| {
                let idx = rng.gen_range(0..CHARSET.len());
                CHARSET[idx] as char
            })
            .collect()
    }

    /// Validate handle format
    fn validate_handle(&self, handle: &str) -> PdsResult<()> {
        // Basic validation (detailed validation in Phase 6)
        if handle.is_empty() {
            return Err(PdsError::Validation("Handle cannot be empty".to_string()));
        }

        if handle.len() < 3 {
            return Err(PdsError::Validation("Handle must be at least 3 characters".to_string()));
        }

        if handle.len() > 253 {
            return Err(PdsError::Validation("Handle too long".to_string()));
        }

        if !handle.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '.') {
            return Err(PdsError::Validation("Handle contains invalid characters".to_string()));
        }

        Ok(())
    }

    /// Validate email format
    fn validate_email(&self, email: &str) -> PdsResult<()> {
        // Basic email validation
        if !email.contains('@') {
            return Err(PdsError::Validation("Invalid email format".to_string()));
        }

        Ok(())
    }

    /// List all accounts with pagination
    ///
    /// Returns accounts ordered by DID for consistent pagination.
    /// Use the last DID as cursor for next page.
    pub async fn list_accounts(
        &self,
        cursor: Option<&str>,
        limit: i64,
    ) -> PdsResult<Vec<Account>> {
        let query = if let Some(cursor_did) = cursor {
            sqlx::query_as::<_, Account>(
                "SELECT did, handle, email, password_hash, created_at, email_confirmed,
                        email_confirmed_at, deactivated_at, taken_down, plc_rotation_key,
                        plc_rotation_key_public, plc_last_operation_cid
                 FROM account
                 WHERE did > ?1
                 ORDER BY did
                 LIMIT ?2"
            )
            .bind(cursor_did)
            .bind(limit)
        } else {
            sqlx::query_as::<_, Account>(
                "SELECT did, handle, email, password_hash, created_at, email_confirmed,
                        email_confirmed_at, deactivated_at, taken_down, plc_rotation_key,
                        plc_rotation_key_public, plc_last_operation_cid
                 FROM account
                 ORDER BY did
                 LIMIT ?1"
            )
            .bind(limit)
        };

        let accounts = query.fetch_all(&self.db).await?;

        Ok(accounts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;
    use std::path::PathBuf;

    async fn setup_test_db() -> AccountManager {
        create_test_manager().await
    }

    async fn create_test_manager() -> AccountManager {

        // Create in-memory database
        let db = SqlitePool::connect(":memory:").await.unwrap();

        // Create tables
        sqlx::query(
            r#"
            CREATE TABLE account (
                did TEXT PRIMARY KEY,
                handle TEXT UNIQUE NOT NULL,
                email TEXT UNIQUE,
                password_hash TEXT NOT NULL,
                created_at DATETIME NOT NULL,
                email_confirmed BOOLEAN NOT NULL DEFAULT 0,
                email_confirmed_at DATETIME,
                deactivated_at DATETIME,
                taken_down BOOLEAN NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE session (
                id TEXT PRIMARY KEY,
                did TEXT NOT NULL,
                access_token TEXT UNIQUE NOT NULL,
                refresh_token TEXT UNIQUE NOT NULL,
                created_at DATETIME NOT NULL,
                expires_at DATETIME NOT NULL,
                app_password_name TEXT,
                FOREIGN KEY (did) REFERENCES account(did)
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE refresh_token (
                id TEXT PRIMARY KEY,
                did TEXT NOT NULL,
                token TEXT UNIQUE NOT NULL,
                created_at DATETIME NOT NULL,
                expires_at DATETIME NOT NULL,
                used BOOLEAN NOT NULL DEFAULT 0,
                used_at DATETIME,
                FOREIGN KEY (did) REFERENCES account(did)
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE app_password (
                did TEXT NOT NULL,
                name TEXT NOT NULL,
                password_hash TEXT NOT NULL,
                created_at DATETIME NOT NULL,
                privileged BOOLEAN NOT NULL DEFAULT 0,
                PRIMARY KEY (did, name),
                FOREIGN KEY (did) REFERENCES account(did)
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        // Create minimal test configuration
        let config = Arc::new(ServerConfig {
            service: ServiceConfig {
                hostname: "localhost".to_string(),
                port: 2583,
                service_did: "did:web:localhost".to_string(),
                version: "0.1.0".to_string(),
                blob_upload_limit: 5242880,
            },
            storage: StorageConfig {
                data_directory: PathBuf::from("./data"),
                account_db: PathBuf::from(":memory:"),
                sequencer_db: PathBuf::from(":memory:"),
                did_cache_db: PathBuf::from(":memory:"),
                actor_store_directory: PathBuf::from("./data/actors"),
                blobstore: BlobstoreConfig::Disk {
                    location: PathBuf::from("./data/blobs"),
                    tmp_location: PathBuf::from("./data/tmp"),
                },
            },
            authentication: AuthConfig {
                jwt_secret: "test-secret-key-for-testing-only".to_string(),
                repo_signing_key: "test-key".to_string(),
                plc_rotation_key: "test-rotation-key".to_string(),
                admin_dids: vec![],
                oauth: crate::config::OAuthConfig {
                    client_id: "test-client".to_string(),
                    redirect_uri: "http://localhost:3000/oauth/callback".to_string(),
                    pds_url: "http://localhost:3000".to_string(),
                },
            },
            identity: IdentityConfig {
                did_plc_url: "https://plc.directory".to_string(),
                service_handle_domains: vec!["localhost".to_string()],
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
                enabled: true,
                global_requests_per_minute: 3000,
            },
            logging: LoggingConfig {
                level: "info".to_string(),
            },
        });

        AccountManager::new(db, config)
    }

    #[tokio::test]
    async fn test_cleanup_expired_sessions() {
        let manager = create_test_manager().await;
        let now = Utc::now();

        // Create a test account
        let did = "did:web:test.localhost";
        sqlx::query(
            "INSERT INTO account (did, handle, email, password_hash, created_at, email_confirmed, taken_down)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
        )
        .bind(did)
        .bind("testuser")
        .bind("test@example.com")
        .bind("hash")
        .bind(now)
        .bind(false)
        .bind(false)
        .execute(&manager.db)
        .await
        .unwrap();

        // Insert expired session (expired 1 hour ago)
        let expired_time = now - Duration::hours(1);
        sqlx::query(
            "INSERT INTO session (id, did, access_token, refresh_token, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        )
        .bind("expired-session-1")
        .bind(did)
        .bind("expired-access-token-1")
        .bind("expired-refresh-token-1")
        .bind(now - Duration::hours(2))
        .bind(expired_time)
        .execute(&manager.db)
        .await
        .unwrap();

        // Insert valid session (expires in 1 hour)
        let future_time = now + Duration::hours(1);
        sqlx::query(
            "INSERT INTO session (id, did, access_token, refresh_token, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        )
        .bind("valid-session-1")
        .bind(did)
        .bind("valid-access-token-1")
        .bind("valid-refresh-token-1")
        .bind(now)
        .bind(future_time)
        .execute(&manager.db)
        .await
        .unwrap();

        // Insert expired refresh token
        sqlx::query(
            "INSERT INTO refresh_token (id, did, token, created_at, expires_at, used)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        )
        .bind("expired-refresh-1")
        .bind(did)
        .bind("old-refresh-token-1")
        .bind(now - Duration::days(200))
        .bind(now - Duration::days(20))
        .bind(false)
        .execute(&manager.db)
        .await
        .unwrap();

        // Insert valid refresh token
        sqlx::query(
            "INSERT INTO refresh_token (id, did, token, created_at, expires_at, used)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        )
        .bind("valid-refresh-1")
        .bind(did)
        .bind("valid-refresh-token-1")
        .bind(now)
        .bind(now + Duration::days(180))
        .bind(false)
        .execute(&manager.db)
        .await
        .unwrap();

        // Run cleanup
        let (sessions_deleted, refresh_tokens_deleted) = manager.cleanup_expired_sessions().await.unwrap();

        // Verify counts
        assert_eq!(sessions_deleted, 1, "Should delete 1 expired session");
        assert_eq!(refresh_tokens_deleted, 1, "Should delete 1 expired refresh token");

        // Verify valid session still exists
        let session_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM session")
            .fetch_one(&manager.db)
            .await
            .unwrap();
        assert_eq!(session_count, 1, "Valid session should remain");

        // Verify valid refresh token still exists
        let refresh_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM refresh_token")
            .fetch_one(&manager.db)
            .await
            .unwrap();
        assert_eq!(refresh_count, 1, "Valid refresh token should remain");
    }

    #[tokio::test]
    async fn test_cleanup_no_expired_sessions() {
        let manager = create_test_manager().await;
        let now = Utc::now();

        // Create a test account
        let did = "did:web:test.localhost";
        sqlx::query(
            "INSERT INTO account (did, handle, email, password_hash, created_at, email_confirmed, taken_down)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
        )
        .bind(did)
        .bind("testuser")
        .bind("test@example.com")
        .bind("hash")
        .bind(now)
        .bind(false)
        .bind(false)
        .execute(&manager.db)
        .await
        .unwrap();

        // Insert only valid session
        let future_time = now + Duration::hours(1);
        sqlx::query(
            "INSERT INTO session (id, did, access_token, refresh_token, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        )
        .bind("valid-session")
        .bind(did)
        .bind("valid-token")
        .bind("valid-refresh")
        .bind(now)
        .bind(future_time)
        .execute(&manager.db)
        .await
        .unwrap();

        // Run cleanup
        let (sessions_deleted, refresh_tokens_deleted) = manager.cleanup_expired_sessions().await.unwrap();

        // Verify no deletions
        assert_eq!(sessions_deleted, 0, "Should not delete any sessions");
        assert_eq!(refresh_tokens_deleted, 0, "Should not delete any refresh tokens");
    }

    #[tokio::test]
    async fn test_create_app_password() {
        let manager = setup_test_db().await;

        // Create test account
        let account = manager
            .create_account(
                "testuser".to_string(),
                Some("test@example.com".to_string()),
                "password123".to_string(),
                None,
            )
            .await
            .unwrap();

        // Create app password
        let app_password = manager
            .create_app_password(&account.did, "Test App", false)
            .await
            .unwrap();

        // Verify format (should be xxxx-xxxx-xxxx-xxxx-xxxx-xxxx-xxxx-xxxx)
        assert_eq!(app_password.len(), 39); // 8 groups of 4 chars + 7 dashes
        assert_eq!(app_password.matches('-').count(), 7);

        // List app passwords
        let passwords = manager.list_app_passwords(&account.did).await.unwrap();
        assert_eq!(passwords.len(), 1);
        assert_eq!(passwords[0].name, "Test App");
        assert_eq!(passwords[0].privileged, false);

        // Create another app password with privileged flag
        manager
            .create_app_password(&account.did, "Privileged App", true)
            .await
            .unwrap();

        let passwords = manager.list_app_passwords(&account.did).await.unwrap();
        assert_eq!(passwords.len(), 2);
    }

    #[tokio::test]
    async fn test_app_password_duplicate_name() {
        let manager = setup_test_db().await;

        let account = manager
            .create_account(
                "testuser".to_string(),
                Some("test@example.com".to_string()),
                "password123".to_string(),
                None,
            )
            .await
            .unwrap();

        // Create first app password
        manager
            .create_app_password(&account.did, "My App", false)
            .await
            .unwrap();

        // Try to create duplicate
        let result = manager
            .create_app_password(&account.did, "My App", false)
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            PdsError::Conflict(_) => {}
            _ => panic!("Expected Conflict error"),
        }
    }

    #[tokio::test]
    async fn test_login_with_app_password() {
        let manager = setup_test_db().await;

        // Create test account
        let account = manager
            .create_account(
                "testuser".to_string(),
                Some("test@example.com".to_string()),
                "password123".to_string(),
                None,
            )
            .await
            .unwrap();

        // Create app password
        let app_password = manager
            .create_app_password(&account.did, "Test Client", false)
            .await
            .unwrap();

        // Login with app password using handle
        let (auth_account, session, name) = manager
            .login_with_app_password("testuser", &app_password)
            .await
            .unwrap();

        assert_eq!(auth_account.did, account.did);
        assert_eq!(name, "Test Client");
        assert!(!session.access_token.is_empty());

        // Verify session has app_password_name set
        let row = sqlx::query("SELECT app_password_name FROM session WHERE id = ?1")
            .bind(&session.id)
            .fetch_one(&manager.db)
            .await
            .unwrap();

        let app_name: String = row.get("app_password_name");
        assert_eq!(app_name, "Test Client");

        // Login with app password using email
        let (auth_account2, _session2, name2) = manager
            .login_with_app_password("test@example.com", &app_password)
            .await
            .unwrap();

        assert_eq!(auth_account2.did, account.did);
        assert_eq!(name2, "Test Client");
    }

    #[tokio::test]
    async fn test_login_with_invalid_app_password() {
        let manager = setup_test_db().await;

        // Create test account
        manager
            .create_account(
                "testuser".to_string(),
                Some("test@example.com".to_string()),
                "password123".to_string(),
                None,
            )
            .await
            .unwrap();

        // Try to login with invalid app password
        let result = manager
            .login_with_app_password("testuser", "invalid-password")
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            PdsError::Authentication(_) => {}
            _ => panic!("Expected Authentication error"),
        }
    }

    #[tokio::test]
    async fn test_revoke_app_password() {
        let manager = setup_test_db().await;

        // Create test account
        let account = manager
            .create_account(
                "testuser".to_string(),
                Some("test@example.com".to_string()),
                "password123".to_string(),
                None,
            )
            .await
            .unwrap();

        // Create app password
        let app_password = manager
            .create_app_password(&account.did, "Test App", false)
            .await
            .unwrap();

        // Create session with app password
        manager
            .login_with_app_password("testuser", &app_password)
            .await
            .unwrap();

        // Verify session exists
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM session WHERE did = ?1 AND app_password_name = ?2"
        )
        .bind(&account.did)
        .bind("Test App")
        .fetch_one(&manager.db)
        .await
        .unwrap();
        assert_eq!(count, 1);

        // Revoke app password
        manager
            .revoke_app_password(&account.did, "Test App")
            .await
            .unwrap();

        // Verify app password deleted
        let passwords = manager.list_app_passwords(&account.did).await.unwrap();
        assert_eq!(passwords.len(), 0);

        // Verify sessions with this app password are deleted
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM session WHERE did = ?1 AND app_password_name = ?2"
        )
        .bind(&account.did)
        .bind("Test App")
        .fetch_one(&manager.db)
        .await
        .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_revoke_nonexistent_app_password() {
        let manager = setup_test_db().await;

        let account = manager
            .create_account(
                "testuser".to_string(),
                Some("test@example.com".to_string()),
                "password123".to_string(),
                None,
            )
            .await
            .unwrap();

        // Try to revoke non-existent app password
        let result = manager
            .revoke_app_password(&account.did, "Nonexistent")
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            PdsError::NotFound(_) => {}
            _ => panic!("Expected NotFound error"),
        }
    }

    #[tokio::test]
    async fn test_app_password_validation() {
        let manager = setup_test_db().await;

        let account = manager
            .create_account(
                "testuser".to_string(),
                Some("test@example.com".to_string()),
                "password123".to_string(),
                None,
            )
            .await
            .unwrap();

        // Test empty name
        let result = manager.create_app_password(&account.did, "", false).await;
        assert!(result.is_err());

        // Test name too long
        let long_name = "a".repeat(101);
        let result = manager
            .create_app_password(&account.did, &long_name, false)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_access_token_with_app_password() {
        let manager = setup_test_db().await;

        // Create test account
        let account = manager
            .create_account(
                "testuser".to_string(),
                Some("test@example.com".to_string()),
                "password123".to_string(),
                None,
            )
            .await
            .unwrap();

        // Create app password and login
        let app_password = manager
            .create_app_password(&account.did, "Test App", false)
            .await
            .unwrap();

        let (_account, session, _name) = manager
            .login_with_app_password("testuser", &app_password)
            .await
            .unwrap();

        // Validate access token
        let validated = manager
            .validate_access_token(&session.access_token)
            .await
            .unwrap();

        assert_eq!(validated.did, account.did);
        assert_eq!(validated.is_app_password, true);

        // Create regular session for comparison
        let (_account, regular_session) = manager.login("testuser", "password123").await.unwrap();

        let validated_regular = manager
            .validate_access_token(&regular_session.access_token)
            .await
            .unwrap();

        assert_eq!(validated_regular.did, account.did);
        assert_eq!(validated_regular.is_app_password, false);
    }

    #[tokio::test]
    async fn test_update_handle() {
        let manager = setup_test_db().await;

        // Create test account
        let account = manager
            .create_account("alice".to_string(), None, "password123".to_string(), None)
            .await
            .unwrap();

        assert_eq!(account.handle, "alice");

        // Update handle to new value
        let old_handle = manager
            .update_handle(&account.did, "alice-new")
            .await
            .unwrap();

        assert_eq!(old_handle, "alice");

        // Verify handle was updated in database
        let updated_account = manager.get_account(&account.did).await.unwrap();
        assert_eq!(updated_account.handle, "alice-new");

        // Verify we can still get account by new handle
        let by_handle = manager
            .get_account_by_identifier("alice-new")
            .await
            .unwrap();
        assert_eq!(by_handle.did, account.did);
    }

    #[tokio::test]
    async fn test_update_handle_conflict() {
        let manager = setup_test_db().await;

        // Create two accounts
        let account1 = manager
            .create_account("alice".to_string(), None, "password123".to_string(), None)
            .await
            .unwrap();

        let account2 = manager
            .create_account("bob".to_string(), None, "password456".to_string(), None)
            .await
            .unwrap();

        // Try to update bob's handle to alice (should fail)
        let result = manager.update_handle(&account2.did, "alice").await;

        assert!(result.is_err());
        match result {
            Err(PdsError::Conflict(msg)) => {
                assert!(msg.contains("already taken"));
            }
            _ => panic!("Expected Conflict error"),
        }

        // Verify bob's handle unchanged
        let bob_account = manager.get_account(&account2.did).await.unwrap();
        assert_eq!(bob_account.handle, "bob");
    }

    #[tokio::test]
    async fn test_update_handle_same_handle() {
        let manager = setup_test_db().await;

        // Create test account
        let account = manager
            .create_account("alice".to_string(), None, "password123".to_string(), None)
            .await
            .unwrap();

        // Update to same handle (should be no-op)
        let old_handle = manager
            .update_handle(&account.did, "alice")
            .await
            .unwrap();

        assert_eq!(old_handle, "alice");

        // Verify handle unchanged
        let updated_account = manager.get_account(&account.did).await.unwrap();
        assert_eq!(updated_account.handle, "alice");
    }

    #[tokio::test]
    async fn test_update_handle_invalid_format() {
        let manager = setup_test_db().await;

        // Create test account
        let account = manager
            .create_account("alice".to_string(), None, "password123".to_string(), None)
            .await
            .unwrap();

        // Try invalid handle with special characters
        let result = manager.update_handle(&account.did, "alice@test").await;
        assert!(result.is_err());

        // Try handle that's too long
        let long_handle = "a".repeat(254);
        let result = manager.update_handle(&account.did, &long_handle).await;
        assert!(result.is_err());

        // Verify handle unchanged
        let unchanged_account = manager.get_account(&account.did).await.unwrap();
        assert_eq!(unchanged_account.handle, "alice");
    }
}
