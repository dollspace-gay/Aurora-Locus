//! Account manager implementation using runtime queries
//! This version uses sqlx runtime query building instead of compile-time macros
//! to avoid needing DATABASE_URL during compilation

use crate::{
    account::AppPasswordInfo,
    config::ServerConfig,
    db::account::{ActorAccount, Session},
    error::{PdsError, PdsResult},
};
use chrono::{DateTime, Duration, Utc};
use sqlx::{AnyPool, Row};
use std::sync::Arc;
use uuid::Uuid;

/// Parse an RFC3339 string from the database into a `DateTime<Utc>`.
/// See chainlink #76 / Phase 3 design notes on chrono ↔ AnyPool.
fn parse_timestamp(s: &str) -> PdsResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))
}

/// Parse `Option<String>` → `Option<DateTime<Utc>>`, propagating parse errors.
fn opt_parse_timestamp(s: Option<String>) -> PdsResult<Option<DateTime<Utc>>> {
    s.as_deref().map(parse_timestamp).transpose()
}


/// Arc 13 §6.3.6 round-4 F2 closure — outcome of
/// [`AccountManager::consume_plc_operation_token`]. Four variants
/// for observability; handler dispatch is two-way (`Consumed` →
/// proceed; everything else → reject as `TokenAlreadyConsumed`).
#[derive(Debug)]
pub enum ConsumeResult {
    /// CAS UPDATE flipped `used: false → true`; the token row
    /// transitioned because of this call.
    Consumed,
    /// CAS UPDATE found zero rows but a follow-up SELECT shows
    /// the token exists and was already used.
    AlreadyConsumed,
    /// CAS UPDATE found zero rows and a follow-up SELECT shows
    /// no matching token row. Effectively impossible if
    /// `validate_plc_operation_token` succeeded immediately
    /// before; logged at warn if seen.
    NotFound,
    /// Database-layer failure on either the UPDATE or the
    /// follow-up SELECT.
    Error(PdsError),
}

/// Build the `<handle>.<domain>`-form full handle, stripping a
/// leading dot off `domain` if present.
///
/// The `service_handle_domains` config defaults to a leading-dot
/// shape (`.{hostname}`) for ATProto-handle suffix matching, so a
/// naive `format!("{}.{}", handle, domain)` produces malformed
/// `usera..localhost`. #69 closed this defect; the strip-or-skip
/// here is the canonical join used by every create_account /
/// did:web fallback site. When the operator-supplied domain has
/// no leading dot, `strip_prefix` returns the original.
fn join_handle_with_domain(handle: &str, domain: &str) -> String {
    let suffix = domain.strip_prefix('.').unwrap_or(domain);
    format!("{}.{}", handle, suffix)
}

/// Account manager service
pub struct AccountManager {
    db: AnyPool,
    config: Arc<ServerConfig>,
}

impl AccountManager {
    /// Create a new account manager
    pub fn new(db: AnyPool, config: Arc<ServerConfig>) -> Self {
        Self { db, config }
    }

    /// Create a new account
    ///
    /// Creates a new actor with associated account credentials.
    /// Inserts into actor, account, and plc_keys tables in a transaction.
    pub async fn create_account(
        &self,
        handle: String,
        email: Option<String>,
        password: String,
        invite_code: Option<String>,
        recovery_key: Option<String>,
    ) -> PdsResult<ActorAccount> {
        // Validate invite code if required
        if self.config.invites.required {
            let code = invite_code
                .as_ref()
                .ok_or_else(|| PdsError::Validation("Invite code required".to_string()))?;
            self.validate_invite_code(code, None).await?;
        }

        // Validate handle format
        self.validate_handle(&handle)?;

        // Validate email if provided
        if let Some(ref email_str) = email {
            self.validate_email(email_str)?;
        }

        // Check if handle already exists
        if self.handle_exists(&handle).await? {
            return Err(PdsError::Conflict(format!(
                "Handle {} already taken",
                handle
            )));
        }

        // Check if email already exists
        if let Some(ref email_str) = email {
            if self.email_exists(email_str).await? {
                return Err(PdsError::Conflict("Email already registered".to_string()));
            }
        }

        // Hash password using SDK's Argon2id implementation
        let password_hash = crate::auth::PasswordHasher::hash(&password)
            .map_err(|e| PdsError::Internal(format!("Password hashing failed: {}", e)))?;

        // Generate DID with PLC registration. Arc 13 §6.3.2 +
        // §6.3.3: returns the per-actor atproto signing key (NOT a
        // per-account rotation key). The per-account recovery_key
        // (when supplied) goes into rotation_keys[0] per §6.3.3
        // priority order.
        let (did, atproto_signing_key_hex, atproto_public_key_hex, plc_operation_cid) =
            self.generate_plc_did(&handle, recovery_key.as_deref()).await?;
        let _ = atproto_public_key_hex; // currently unused at insert site

        let now = Utc::now();

        // Begin transaction to insert into multiple tables atomically
        let mut tx = self.db.begin().await.map_err(PdsError::Database)?;

        // Insert into actor table (public identity)
        sqlx::query(
            "INSERT INTO actor (did, handle, created_at, takedown_ref, deactivated_at, delete_after)
             VALUES ($1, $2, $3, NULL, NULL, NULL)"
        )
        .bind(&did)
        .bind(&handle)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(PdsError::Database)?;

        // Insert into account table (private auth)
        sqlx::query(
            "INSERT INTO account (did, email, password_hash, email_confirmed_at, invites_disabled)
             VALUES ($1, $2, $3, NULL, FALSE)",
        )
        .bind(&did)
        .bind(&email)
        .bind(&password_hash)
        .execute(&mut *tx)
        .await
        .map_err(PdsError::Database)?;

        // Insert into plc_keys table (cryptographic material).
        // Arc 13 §6.4 Step 0.7.1: `rotation_key` +
        // `rotation_key_public` columns dropped — the PDS-wide
        // rotation key lives in config, not per-account state.
        // Only the per-actor atproto signing key returned from
        // `generate_plc_did` is persisted here.
        sqlx::query(
            "INSERT INTO plc_keys (did, last_operation_cid, atproto_signing_key)
             VALUES ($1, $2, $3)",
        )
        .bind(&did)
        .bind(&plc_operation_cid)
        .bind(&atproto_signing_key_hex)
        .execute(&mut *tx)
        .await
        .map_err(PdsError::Database)?;

        // Commit transaction
        tx.commit().await.map_err(PdsError::Database)?;

        // Use invite code if provided
        if let Some(code) = invite_code {
            if self.config.invites.required {
                self.use_invite_code(&code, &did).await?;
            }
        }

        // Return combined ActorAccount
        Ok(ActorAccount {
            // Actor fields
            did,
            handle: Some(handle),
            created_at: now,
            takedown_ref: None,
            deactivated_at: None,
            delete_after: None,
            suspended_at: None,
            desynchronized_at: None,
            // Account fields
            email,
            password_hash: Some(password_hash),
            email_confirmed_at: None,
            invites_disabled: Some(false),
        })
    }

    /// Authenticate account and create session
    ///
    /// This function includes timing attack protection by ensuring a minimum
    /// execution time of 350ms to prevent username enumeration via timing analysis.
    pub async fn login(
        &self,
        identifier: &str,
        password: &str,
    ) -> PdsResult<(ActorAccount, Session)> {
        // Start timing for attack mitigation
        let start = std::time::Instant::now();

        // Perform login logic - wrap in a scope so we can handle timing in finally block
        let result = async {
            // Find account by handle or email
            let account = self.get_account_by_identifier(identifier).await?;

            // Check if account is deactivated or taken down
            if account.deactivated_at.is_some() {
                return Err(PdsError::Authorization(
                    "Account is deactivated".to_string(),
                ));
            }

            if account.takedown_ref.is_some() {
                return Err(PdsError::Authorization(
                    "Account has been taken down".to_string(),
                ));
            }

            // Verify password exists (must have local account)
            let password_hash = account.password_hash.as_ref().ok_or_else(|| {
                PdsError::Authentication("No local account credentials".to_string())
            })?;

            // Verify password
            let valid = crate::auth::PasswordHasher::verify(password, password_hash)
                .map_err(|e| PdsError::Internal(format!("Password verification failed: {}", e)))?;

            if !valid {
                return Err(PdsError::Authentication("Invalid credentials".to_string()));
            }

            // Create session
            let session = self.create_session(&account.did, None).await?;

            Ok((account, session))
        }
        .await;

        // Mitigate timing attacks by ensuring minimum execution time
        // This prevents attackers from distinguishing valid vs invalid usernames
        // based on response time differences
        let elapsed = start.elapsed().as_millis() as i64;
        let wait_time = 350 - elapsed;
        if wait_time > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(wait_time as u64)).await;
        }

        result
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
             VALUES ($1, $2, $3, $4, $5, $6, $7)"
        )
        .bind(&session_id)
        .bind(did)
        .bind(&access_token)
        .bind(&refresh_token_str)
        .bind(now.to_rfc3339())
        .bind(expires_at.to_rfc3339())
        .bind(&app_password_name)
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;

        // Store refresh token
        let refresh_token_id = Uuid::new_v4().to_string();
        let refresh_expires = now + Duration::days(180); // Refresh token expires in 6 months

        sqlx::query(
            "INSERT INTO refresh_token (id, did, token, created_at, expires_at, used, next_id)
             VALUES ($1, $2, $3, $4, $5, $6, NULL)",
        )
        .bind(&refresh_token_id)
        .bind(did)
        .bind(&refresh_token_str)
        .bind(now.to_rfc3339())
        .bind(refresh_expires.to_rfc3339())
        .bind(false)
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;

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
    pub async fn validate_access_token(
        &self,
        token: &str,
    ) -> PdsResult<crate::account::ValidatedSession> {
        // Find session by access token
        let row = sqlx::query(
            "SELECT id, did, expires_at, app_password_name FROM session WHERE access_token = $1",
        )
        .bind(token)
        .fetch_optional(&self.db)
        .await
        .map_err(PdsError::Database)?
        .ok_or_else(|| PdsError::Authentication("Invalid or expired session".to_string()))?;

        let session_id: String = row.get("id");
        let did: String = row.get("did");
        let expires_at: DateTime<Utc> = parse_timestamp(&row.get::<String, _>("expires_at"))?;
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
        sqlx::query("DELETE FROM session WHERE id = $1")
            .bind(session_id)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

        Ok(())
    }

    /// Refresh session tokens with 2-hour grace period
    ///
    /// Implements token rotation with a grace period to handle concurrent refresh requests.
    /// When a token is refreshed, the old token remains valid for 2 hours but points to
    /// the new token, preventing race conditions.
    pub async fn refresh_session(&self, refresh_token: &str) -> PdsResult<Session> {
        let now = Utc::now();

        // Find and validate refresh token
        let row = sqlx::query(
            "SELECT id, did, token, created_at, expires_at, used, used_at, next_id FROM refresh_token WHERE token = $1"
        )
        .bind(refresh_token)
        .fetch_optional(&self.db)
        .await
        .map_err(PdsError::Database)?
        .ok_or_else(|| PdsError::Authentication("Invalid refresh token".to_string()))?;

        let _token_id: String = row.get("id");
        let did: String = row.get("did");
        let expires_at: DateTime<Utc> = parse_timestamp(&row.get::<String, _>("expires_at"))?;
        let used: bool = crate::db::read_bool(&row, "used")?;
        let next_id: Option<String> = row.get("next_id");

        // Check expiration
        if now > expires_at {
            return Err(PdsError::Authentication(
                "Refresh token expired".to_string(),
            ));
        }

        // If token was already used but has a next_id (grace period scenario)
        if used {
            if let Some(next_token_id) = next_id {
                // Return the same new session that was created before (within grace period)
                // This handles concurrent refresh attempts
                let next_row = sqlx::query(
                    "SELECT s.id, s.did, s.access_token, s.refresh_token, s.created_at, s.expires_at, s.app_password_name
                     FROM refresh_token rt
                     JOIN session s ON s.refresh_token = (SELECT token FROM refresh_token WHERE id = $1)
                     WHERE rt.id = $1"
                )
                .bind(&next_token_id)
                .fetch_optional(&self.db)
                .await
                .map_err(PdsError::Database)?;

                if let Some(session_row) = next_row {
                    return Ok(Session {
                        id: session_row.get("id"),
                        did: session_row.get("did"),
                        access_token: session_row.get("access_token"),
                        refresh_token: session_row.get("refresh_token"),
                        created_at: parse_timestamp(&session_row.get::<String, _>("created_at"))?,
                        expires_at: parse_timestamp(&session_row.get::<String, _>("expires_at"))?,
                        app_password_name: session_row.get("app_password_name"),
                    });
                }
            }
            return Err(PdsError::Authentication(
                "Refresh token already used".to_string(),
            ));
        }

        // Create new refresh token
        let new_token_id = uuid::Uuid::new_v4().to_string();
        let new_refresh_token = self.generate_refresh_token(&did, &new_token_id)?;
        let refresh_expires = now + Duration::days(180); // 180 days

        // Insert new refresh token
        sqlx::query(
            "INSERT INTO refresh_token (id, did, token, created_at, expires_at, used, next_id)
             VALUES ($1, $2, $3, $4, $5, $6, NULL)",
        )
        .bind(&new_token_id)
        .bind(&did)
        .bind(&new_refresh_token)
        .bind(now.to_rfc3339())
        .bind(refresh_expires.to_rfc3339())
        .bind(false)
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;

        // Update old refresh token: mark as used, set next_id, and shorten expiration to 2 hours
        let grace_period_expires = now + Duration::hours(2);
        sqlx::query(
            "UPDATE refresh_token SET used = TRUE, used_at = $1, next_id = $2, expires_at = $3 WHERE id = $4"
        )
        .bind(now.to_rfc3339())
        .bind(&new_token_id)
        .bind(grace_period_expires.to_rfc3339())
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;

        // Create new access token
        let new_session_id = uuid::Uuid::new_v4().to_string();
        let access_token = self.generate_access_token(&did, &new_session_id)?;
        let access_expires = now + Duration::hours(1);

        // Insert new session
        sqlx::query(
            "INSERT INTO session (id, did, access_token, refresh_token, created_at, expires_at, app_password_name)
             VALUES ($1, $2, $3, $4, $5, $6, NULL)"
        )
        .bind(&new_session_id)
        .bind(&did)
        .bind(&access_token)
        .bind(&new_refresh_token)
        .bind(now.to_rfc3339())
        .bind(access_expires.to_rfc3339())
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;

        Ok(Session {
            id: new_session_id,
            did: did.to_string(),
            access_token,
            refresh_token: new_refresh_token,
            created_at: now,
            expires_at: access_expires,
            app_password_name: None,
        })
    }

    /// Get account by DID
    ///
    /// Joins actor and account tables to get complete actor information.
    /// Arc 15 §8.3.8 / Step 6: load the actor's `atproto_signing_key`
    /// (per Arc 13 §6.3.2 key separation) from `plc_keys`. Used by
    /// `create_account_emit_sequence` to construct the genesis-commit
    /// Signer. Returns the 32-byte private-key bytes (decoded from
    /// hex). Errors if the row is missing or the hex is malformed.
    pub async fn get_atproto_signing_key_bytes(&self, did: &str) -> PdsResult<Vec<u8>> {
        let row = sqlx::query("SELECT atproto_signing_key FROM plc_keys WHERE did = $1")
            .bind(did)
            .fetch_optional(&self.db)
            .await
            .map_err(PdsError::Database)?
            .ok_or_else(|| {
                PdsError::NotFound(format!("plc_keys row missing for did={}", did))
            })?;
        let hex_key: String = row.try_get("atproto_signing_key").map_err(PdsError::Database)?;
        hex::decode(&hex_key).map_err(|e| {
            PdsError::Internal(format!("atproto_signing_key for {} is malformed hex: {}", did, e))
        })
    }

    pub async fn get_account(&self, did: &str) -> PdsResult<ActorAccount> {
        let row = sqlx::query(
            "SELECT
                a.did, a.handle, a.created_at, a.takedown_ref, a.deactivated_at, a.delete_after,
                a.suspended_at, a.desynchronized_at,
                ac.email, ac.password_hash, ac.email_confirmed_at, ac.invites_disabled
             FROM actor a
             LEFT JOIN account ac ON a.did = ac.did
             WHERE a.did = $1",
        )
        .bind(did)
        .fetch_optional(&self.db)
        .await
        .map_err(PdsError::Database)?
        .ok_or_else(|| PdsError::NotFound("Account not found".to_string()))?;

        Ok(ActorAccount {
            // Actor fields
            did: row.get("did"),
            handle: row.get("handle"),
            created_at: parse_timestamp(&row.get::<String, _>("created_at"))?,
            takedown_ref: row.get("takedown_ref"),
            deactivated_at: opt_parse_timestamp(row.get::<Option<String>, _>("deactivated_at"))?,
            delete_after: opt_parse_timestamp(row.get::<Option<String>, _>("delete_after"))?,
            // Arc 14 §7.3.6: suspended/desynchronized timestamps.
            suspended_at: opt_parse_timestamp(row.get::<Option<String>, _>("suspended_at"))?,
            desynchronized_at: opt_parse_timestamp(row.get::<Option<String>, _>("desynchronized_at"))?,
            // Account fields (may be None for federated actors)
            email: row.get("email"),
            password_hash: row.get("password_hash"),
            email_confirmed_at: opt_parse_timestamp(row.get::<Option<String>, _>("email_confirmed_at"))?,
            invites_disabled: Some(crate::db::read_bool(&row, "invites_disabled")?),
        })
    }

    /// Find account by DID, handle, or email (public for password reset).
    pub async fn get_account_by_identifier(&self, identifier: &str) -> PdsResult<ActorAccount> {
        // v0.8 arc 3 (#184) — DID-form identifier. atproto's createSession
        // identifier accepts a DID; route it straight to the by-DID lookup
        // (`get_account`). The `did:` prefix is unambiguous — handles and
        // emails can never start `did:` (the handle charset rule and the
        // email `:`-reject forbid it; M4/M5) — so this is DID-lookup-only
        // with NO handle/email fallback after a miss: a `did:`-prefixed miss
        // is a real miss.
        if identifier.starts_with("did:") {
            let result = self.get_account(identifier).await;
            if matches!(&result, Err(PdsError::NotFound(_))) {
                // Forensic anchor — a DID identifier that misses means the
                // DID genuinely has no local account (vs. a typo'd handle).
                // debug-level: silent under the default `aurora_locus=info`
                // filter; visible with RUST_LOG=aurora_locus::auth=debug.
                tracing::debug!(
                    target: "aurora_locus::auth",
                    event = "login_did_identifier_miss",
                    did = %identifier,
                    "DID identifier did not resolve to a local account",
                );
            }
            return result;
        }

        // Try handle first
        if let Ok(account) = self.get_account_by_handle(identifier).await {
            return Ok(account);
        }

        // Try email
        self.get_account_by_email(identifier).await
    }

    /// Resolve an at-identifier (handle or DID) to a canonical DID.
    ///
    /// DID-form input is returned as-is without any DB lookup (the caller
    /// usually wants to perform the actual DB operation against a DID
    /// regardless of whether the account exists locally — e.g., a takedown
    /// of a federated DID).
    ///
    /// Handle-form input is resolved via local actor-table lookup. External
    /// handle resolution (DNS / .well-known) is *not* performed: admin
    /// endpoints operate on the local PDS's accounts, so a handle that
    /// doesn't match any local actor returns `PdsError::NotFound`.
    pub async fn resolve_at_identifier_to_did(&self, identifier: &str) -> PdsResult<String> {
        if identifier.starts_with("did:") {
            return Ok(identifier.to_string());
        }
        self.get_account_by_handle(identifier)
            .await
            .map(|acc| acc.did)
    }

    /// Get account by handle
    ///
    /// Joins actor and account tables to get complete actor information.
    async fn get_account_by_handle(&self, handle: &str) -> PdsResult<ActorAccount> {
        let row = sqlx::query(
            "SELECT
                a.did, a.handle, a.created_at, a.takedown_ref, a.deactivated_at, a.delete_after,
                a.suspended_at, a.desynchronized_at,
                ac.email, ac.password_hash, ac.email_confirmed_at, ac.invites_disabled
             FROM actor a
             LEFT JOIN account ac ON a.did = ac.did
             WHERE a.handle = $1",
        )
        .bind(handle)
        .fetch_optional(&self.db)
        .await
        .map_err(PdsError::Database)?
        .ok_or_else(|| PdsError::NotFound("Actor not found".to_string()))?;

        Ok(ActorAccount {
            // Actor fields
            did: row.get("did"),
            handle: row.get("handle"),
            created_at: parse_timestamp(&row.get::<String, _>("created_at"))?,
            takedown_ref: row.get("takedown_ref"),
            deactivated_at: opt_parse_timestamp(row.get::<Option<String>, _>("deactivated_at"))?,
            delete_after: opt_parse_timestamp(row.get::<Option<String>, _>("delete_after"))?,
            suspended_at: opt_parse_timestamp(row.get::<Option<String>, _>("suspended_at"))?,
            desynchronized_at: opt_parse_timestamp(row.get::<Option<String>, _>("desynchronized_at"))?,
            // Account fields (may be None for federated actors)
            email: row.get("email"),
            password_hash: row.get("password_hash"),
            email_confirmed_at: opt_parse_timestamp(row.get::<Option<String>, _>("email_confirmed_at"))?,
            invites_disabled: Some(crate::db::read_bool(&row, "invites_disabled")?),
        })
    }

    /// Get account by email
    ///
    /// Joins actor and account tables to get complete actor information.
    async fn get_account_by_email(&self, email: &str) -> PdsResult<ActorAccount> {
        let row = sqlx::query(
            "SELECT
                a.did, a.handle, a.created_at, a.takedown_ref, a.deactivated_at, a.delete_after,
                a.suspended_at, a.desynchronized_at,
                ac.email, ac.password_hash, ac.email_confirmed_at, ac.invites_disabled
             FROM actor a
             INNER JOIN account ac ON a.did = ac.did
             WHERE ac.email = $1",
        )
        .bind(email)
        .fetch_optional(&self.db)
        .await
        .map_err(PdsError::Database)?
        .ok_or_else(|| PdsError::NotFound("Account not found".to_string()))?;

        Ok(ActorAccount {
            // Actor fields
            did: row.get("did"),
            handle: row.get("handle"),
            created_at: parse_timestamp(&row.get::<String, _>("created_at"))?,
            takedown_ref: row.get("takedown_ref"),
            deactivated_at: opt_parse_timestamp(row.get::<Option<String>, _>("deactivated_at"))?,
            delete_after: opt_parse_timestamp(row.get::<Option<String>, _>("delete_after"))?,
            suspended_at: opt_parse_timestamp(row.get::<Option<String>, _>("suspended_at"))?,
            desynchronized_at: opt_parse_timestamp(row.get::<Option<String>, _>("desynchronized_at"))?,
            // Account fields
            email: row.get("email"),
            password_hash: row.get("password_hash"),
            email_confirmed_at: opt_parse_timestamp(row.get::<Option<String>, _>("email_confirmed_at"))?,
            invites_disabled: Some(crate::db::read_bool(&row, "invites_disabled")?),
        })
    }

    /// Check if handle exists
    async fn handle_exists(&self, handle: &str) -> PdsResult<bool> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM actor WHERE handle = $1")
            .bind(handle)
            .fetch_one(&self.db)
            .await
            .map_err(PdsError::Database)?;

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
        let old_handle = account.handle.clone().unwrap_or_default();

        // Check if new handle is the same as current (no-op)
        if account.handle.as_deref() == Some(new_handle) {
            return Ok(old_handle);
        }

        // Check if new handle is already taken by another account
        if let Ok(existing) = self.get_account_by_handle(new_handle).await {
            if existing.did != did {
                return Err(PdsError::Conflict(format!(
                    "Handle {} already taken",
                    new_handle
                )));
            }
        }

        let mut tx = self.db.begin().await.map_err(PdsError::Database)?;
        Self::update_handle_unchecked_in_tx(&mut tx, did, new_handle).await?;
        tx.commit().await.map_err(PdsError::Database)?;
        Ok(old_handle)
    }

    /// Update handle inside an existing transaction. LB-1 / chainlink #129
    /// atomic-with-chain entry point. Returns the old handle.
    ///
    /// Caller is responsible for handle-format validation upstream of
    /// this call — the in-tx variant skips the
    /// `crate::identity::validate_handle` check (which needs access to
    /// `&self.config` for the allowed service-handle-domain list) and
    /// trusts the caller. The conflict check + UPDATE both run inside
    /// the transaction so the read of `actor` and the subsequent
    /// UPDATE see the same snapshot.
    pub async fn update_handle_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        did: &str,
        new_handle: &str,
    ) -> PdsResult<String> {
        // Read old handle inside tx so the snapshot is consistent
        // with the subsequent UPDATE.
        let old_row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT handle FROM actor WHERE did = $1")
                .bind(did)
                .fetch_optional(&mut **tx)
                .await
                .map_err(PdsError::Database)?;
        let old_handle = old_row
            .ok_or_else(|| PdsError::NotFound(format!("Account not found: {}", did)))?
            .0
            .unwrap_or_default();

        if old_handle == new_handle {
            return Ok(old_handle);
        }

        // Check if new handle is already taken by another account.
        let conflict: Option<(String,)> =
            sqlx::query_as("SELECT did FROM actor WHERE handle = $1 AND did != $2")
                .bind(new_handle)
                .bind(did)
                .fetch_optional(&mut **tx)
                .await
                .map_err(PdsError::Database)?;
        if conflict.is_some() {
            return Err(PdsError::Conflict(format!(
                "Handle {} already taken",
                new_handle
            )));
        }

        Self::update_handle_unchecked_in_tx(tx, did, new_handle).await?;
        Ok(old_handle)
    }

    /// Apply the actual UPDATE without re-running validation. Used by
    /// `update_handle` (which validates upfront) and
    /// `update_handle_in_tx` (which validates inside the tx).
    async fn update_handle_unchecked_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        did: &str,
        new_handle: &str,
    ) -> PdsResult<()> {
        sqlx::query("UPDATE actor SET handle = $1 WHERE did = $2")
            .bind(new_handle)
            .bind(did)
            .execute(&mut **tx)
            .await
            .map_err(PdsError::Database)?;
        Ok(())
    }

    /// Check if email exists
    async fn email_exists(&self, email: &str) -> PdsResult<bool> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM account WHERE email = $1")
            .bind(email)
            .fetch_one(&self.db)
            .await
            .map_err(PdsError::Database)?;

        Ok(count > 0)
    }

    /// Generate DID for handle
    /// Generate a PLC DID and register it with the PLC Directory
    ///
    /// Returns: (did, rotation_key_hex, rotation_key_public_hex, operation_cid)
    async fn generate_plc_did(
        &self,
        handle: &str,
        recovery_key: Option<&str>,
    ) -> PdsResult<(String, String, String, String)> {
        use crate::crypto::plc::{
            compute_op_cid, derive_did_suffix, register_plc_did, PlcOperationBuilder, PlcSigner,
            ServiceEntry,
        };
        use rand::RngCore;
        use std::collections::BTreeMap;

        // §6.3.7 + §6.6.6 — hard-fail on PLC registration failure.
        // In #[cfg(test)] builds we short-circuit the actual PLC
        // HTTP call so unit tests that need accounts (~22 tests
        // pre-Step-5 relied on the silent did:web fallback) don't
        // require a running mock PLC directory. The hard-fail
        // behavior itself is verified by Phase B Scenario 6
        // (§6.8.2) which is operator-driven.
        #[cfg(test)]
        let test_short_circuit_did_plc_url = {
            // If the test fixture points at the prod PLC URL,
            // synthesize a fake DID + signing key. Real PLC tests
            // (Scenario 6) point at an unreachable URL and assert
            // the hard-fail error explicitly via the production
            // path (no #[cfg(test)] short-circuit applies).
            //
            // Treat https://plc.directory and 127.0.0.1:0 as
            // "synthesize" markers; any other URL goes through
            // the real path.
            let url = self.config.identity.did_plc_url.as_str();
            url == "https://plc.directory" || url.contains("127.0.0.1:0")
        };
        #[cfg(test)]
        if test_short_circuit_did_plc_url {
            let mut atproto_private_key = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut atproto_private_key);
            let atproto_signing_key_hex = hex::encode(atproto_private_key);
            let atproto_signer = PlcSigner::new(&atproto_private_key)?;
            let atproto_public_key_hex = atproto_signer.public_key_hex();
            // Synthesize a stable-shape DID for test fixtures.
            let synthetic_suffix: String = atproto_public_key_hex
                .chars()
                .take(24)
                .collect();
            let did = format!("did:plc:{}", synthetic_suffix);
            let _ = recovery_key;
            let _ = handle;
            return Ok((
                did,
                atproto_signing_key_hex,
                atproto_public_key_hex,
                String::new(),
            ));
        }

        // §6.3.2 key separation: the PDS-wide rotation key signs
        // every account's genesis op (and every later update op).
        // It comes from `config.authentication.plc_rotation_key`
        // — one key per PDS deployment, loaded once at startup,
        // shared across every account. Its `did:key` URI is what
        // ends up in `rotation_keys[N-1]` so the signer's key
        // satisfies the spec-required invariant from chainlink
        // #61 §1.4.5.
        let rotation_signer =
            PlcSigner::from_hex(&self.config.authentication.plc_rotation_key)?;
        let rotation_did_key = rotation_signer.public_key_did_key();

        // §6.3.2 key separation: the per-actor atproto signing key
        // is a *separate* fresh ES256K key, generated here per
        // account. Its `did:key` URI goes into
        // `verification_methods["atproto"]` and it's stored in
        // `plc_keys.atproto_signing_key` for later use by
        // `entryway_auth_headers` (Arc 12 §5.3.5) + repo commit
        // signing.
        let mut atproto_private_key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut atproto_private_key);
        let atproto_signing_key_hex = hex::encode(atproto_private_key);
        let atproto_signer = PlcSigner::new(&atproto_private_key)?;
        let atproto_public_key_hex = atproto_signer.public_key_hex();
        let atproto_did_key = atproto_signer.public_key_did_key();

        // Arc 12 §5.3.2 Gap 1 closure: read effective_public_url()
        // so localhost / explicit PDS_SERVICE_PUBLIC_URL deployments
        // produce the correct scheme + port rather than baking
        // `https://hostname` (no port) into the immutable PLC
        // genesis op CBOR.
        let service_url = self.config.service.effective_public_url();

        // Handle-to-full-handle. #69 closed: the dot-strip below
        // prevents `usera..localhost` when service_handle_domains
        // entries carry a leading dot (the config default shape).
        let full_handle = if handle.contains('.')
            && self
                .config
                .identity
                .service_handle_domains
                .iter()
                .any(|d| handle.ends_with(d))
        {
            handle.to_string()
        } else {
            join_handle_with_domain(
                handle,
                &self.config.identity.service_handle_domains[0],
            )
        };

        // Arc 13 §6.3.1 wire-shape: services as map keyed by
        // service name (`atproto_pds`), verification_methods as
        // map keyed by purpose name (`atproto`), rotation_keys
        // as plain Vec.
        let mut services = BTreeMap::new();
        services.insert(
            "atproto_pds".to_string(),
            ServiceEntry {
                type_: "AtprotoPersonalDataServer".to_string(),
                endpoint: service_url,
            },
        );

        // §6.3.2 mapping: verification_methods["atproto"] points
        // at the per-actor signing key (NOT the rotation key).
        let mut verification_methods = BTreeMap::new();
        verification_methods.insert("atproto".to_string(), atproto_did_key);

        // §6.3.3 Step 2.3 priority order: rotation_keys =
        // [input.recovery_key?, config.recovery_did_key?,
        //  config.plc_rotation_key.did_key()].
        // Earlier entries in the list have higher rotation
        // authority (operator/account-owned recovery keys can
        // override the PDS server's signing).
        let mut rotation_keys: Vec<String> = Vec::with_capacity(3);
        if let Some(per_account) = recovery_key {
            let trimmed = per_account.trim();
            if !trimmed.is_empty() {
                rotation_keys.push(trimmed.to_string());
            }
        }
        if let Some(pds_recovery) = &self.config.identity.recovery_did_key {
            if !pds_recovery.is_empty() {
                rotation_keys.push(pds_recovery.clone());
            }
        }
        rotation_keys.push(rotation_did_key);

        let unsigned = PlcOperationBuilder::new()
            .rotation_keys(rotation_keys)
            .verification_methods(verification_methods)
            .also_known_as(vec![format!("at://{}", full_handle)])
            .services(services)
            .build()?;

        // §6.3.1 / Step 0.6.1: DID suffix is SHA-256 of canonical
        // DAG-CBOR of unsigned op, base32-lower (no padding), first
        // 24 chars.
        let did_suffix = derive_did_suffix(&unsigned)?;
        let did = format!("did:plc:{}", did_suffix);

        // §6.3.2: the PDS-wide rotation key signs (its `did:key`
        // is in `rotation_keys[0]`, satisfying chainlink #61 §1.4.5
        // signer-in-rotation-keys invariant).
        let signed_operation = rotation_signer.sign_operation(unsigned)?;

        let plc_url = self.config.identity.did_plc_url.as_str();

        match register_plc_did(plc_url, &did, signed_operation.clone()).await {
            Ok(_) => {
                tracing::info!("Successfully registered DID with PLC directory: {}", did);

                // §6.3.1 / Step 0.6.2: CID over canonical DAG-CBOR
                // of signed op (the proper PLC-spec CID).
                let operation_cid = compute_op_cid(&signed_operation)?;

                // Return shape: (did, atproto_signing_key_hex,
                // atproto_public_key_hex, operation_cid). The
                // rotation key isn't returned — it's the PDS-wide
                // key, stored in config, not per-account state.
                Ok((
                    did,
                    atproto_signing_key_hex,
                    atproto_public_key_hex,
                    operation_cid,
                ))
            }
            Err(e) => {
                // §6.3.7 / §6.4 Step 5 — hard-fail. Silent
                // did:web fallback removed. PLC directory
                // unreachable → no account creation succeeds.
                //
                // Partial-state cleanup: generate_plc_did is
                // called BEFORE create_account opens its
                // transaction (line 88-89 of create_account),
                // so no DB rows have been inserted at this
                // point. The early return from create_account
                // leaves no partial actor state to clean up.
                // The PDS-wide rotation key is unaffected (it's
                // config-resident, not allocated here). The
                // per-actor atproto_signing_key generated above
                // is in stack memory only; dropped when the
                // function returns.
                tracing::error!(
                    did = %did,
                    handle = %full_handle,
                    error = %e,
                    "PLC directory registration failed; hard-failing account creation per §6.3.7"
                );
                Err(e)
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

        // Arc 12 §5.4 Step 0.6.2: include kid="aurora-local-v1"
        // in JWT header so tuple-routing per §5.3.3 can route
        // local-mint tokens to the local-verify path
        // unambiguously. Pre-Step-0.6 kid-less tokens still
        // route to local-verify by HS256+kid-absent rule per
        // §5.3.3 tuple table; the kid here makes the routing
        // explicit + tracks issuance for future revocation
        // surfaces (§5.5.2).
        let header = Header {
            kid: Some("aurora-local-v1".to_string()),
            ..Header::default()
        };

        let token = encode(
            &header,
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
        let sessions_result = sqlx::query("DELETE FROM session WHERE expires_at < $1")
            .bind(now.to_rfc3339())
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

        let sessions_deleted = sessions_result.rows_affected();

        // Delete expired refresh tokens
        let refresh_result = sqlx::query("DELETE FROM refresh_token WHERE expires_at < $1")
            .bind(now.to_rfc3339())
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

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
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(&token)
        .bind(did)
        .bind("confirm_email")
        .bind(now.to_rfc3339())
        .bind(expires_at.to_rfc3339())
        .bind(false)
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;

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
            WHERE token = $1 AND purpose = 'confirm_email'
            "#,
        )
        .bind(token)
        .fetch_optional(&self.db)
        .await
        .map_err(PdsError::Database)?
        .ok_or_else(|| PdsError::NotFound("Invalid verification token".to_string()))?;

        let did: String = row.try_get("did")?;
        let expires_at: DateTime<Utc> = parse_timestamp(&row.try_get::<String, _>("expires_at")?)?;
        // chainlink #74 / #86: sqlx::Any bool/BIGINT mismatch fix —
        // same pattern as #71 closure for validate_plc_operation_token.
        let used: bool = crate::db::read_bool(&row, "used")?;

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
        sqlx::query("UPDATE email_token SET used = true WHERE token = $1")
            .bind(token)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

        // Mark email as confirmed in account (only update email_confirmed_at)
        sqlx::query("UPDATE account SET email_confirmed_at = $1 WHERE did = $2")
            .bind(now.to_rfc3339())
            .bind(&did)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

        tracing::info!("Email confirmed for DID: {}", did);

        Ok(did)
    }

    /// Request new email confirmation
    ///
    /// Generates a new token and can optionally send verification email
    pub async fn request_email_confirmation(&self, did: &str) -> PdsResult<String> {
        // Verify account exists and has email
        let row = sqlx::query("SELECT email FROM account WHERE did = $1")
            .bind(did)
            .fetch_optional(&self.db)
            .await
            .map_err(PdsError::Database)?
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
    pub async fn generate_password_reset_token(
        &self,
        identifier: &str,
    ) -> PdsResult<(String, String)> {
        // Find account by email or handle (read outside tx for simplicity).
        let account = self.get_account_by_identifier(identifier).await?;
        if account.email.is_none() {
            return Err(PdsError::Validation(
                "Account does not have an email address".to_string(),
            ));
        }
        let mut tx = self.db.begin().await.map_err(PdsError::Database)?;
        let token =
            Self::generate_password_reset_token_in_tx(&mut tx, &account.did).await?;
        tx.commit().await.map_err(PdsError::Database)?;
        Ok((token, account.email.unwrap()))
    }

    /// Generate and store a password-reset token for `did` inside an
    /// existing transaction. LB-1 / chainlink #129 atomic-with-chain
    /// entry point. Caller is responsible for verifying the account
    /// exists and has an email upstream of this call. Returns the
    /// generated token.
    pub async fn generate_password_reset_token_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        did: &str,
    ) -> PdsResult<String> {
        let token = Uuid::new_v4().to_string();
        let now = Utc::now();
        let expires_at = now + Duration::hours(1);

        sqlx::query(
            r#"
            INSERT INTO email_token (token, did, purpose, created_at, expires_at, used)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(&token)
        .bind(did)
        .bind("reset_password")
        .bind(now.to_rfc3339())
        .bind(expires_at.to_rfc3339())
        .bind(false)
        .execute(&mut **tx)
        .await
        .map_err(PdsError::Database)?;

        Ok(token)
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
            WHERE token = $1 AND purpose = 'reset_password'
            "#,
        )
        .bind(token)
        .fetch_optional(&self.db)
        .await
        .map_err(PdsError::Database)?
        .ok_or_else(|| PdsError::NotFound("Invalid reset token".to_string()))?;

        let did: String = row.try_get("did")?;
        let expires_at: DateTime<Utc> = parse_timestamp(&row.try_get::<String, _>("expires_at")?)?;
        // chainlink #74 / #86: sqlx::Any bool/BIGINT mismatch fix.
        let used: bool = crate::db::read_bool(&row, "used")?;

        // Check if already used
        if used {
            return Err(PdsError::Validation(
                "Reset token has already been used".to_string(),
            ));
        }

        // Check expiration
        if now > expires_at {
            return Err(PdsError::Validation("Reset token has expired".to_string()));
        }

        // Hash new password
        let password_hash = crate::auth::PasswordHasher::hash(new_password)
            .map_err(|e| PdsError::Internal(format!("Password hashing failed: {}", e)))?;

        // Update password in database
        sqlx::query("UPDATE account SET password_hash = $1 WHERE did = $2")
            .bind(&password_hash)
            .bind(&did)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

        // Mark token as used
        sqlx::query("UPDATE email_token SET used = true WHERE token = $1")
            .bind(token)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

        // Invalidate all sessions for this account (security best practice)
        sqlx::query("DELETE FROM session WHERE did = $1")
            .bind(&did)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

        // Also delete all refresh tokens
        sqlx::query("DELETE FROM refresh_token WHERE did = $1")
            .bind(&did)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

        tracing::info!("Password reset successful for DID: {}", did);

        Ok(())
    }

    // ============================================================
    // Arc 13 §6.3.6 + Step 3.1 — `plc_operation` email-token surface.
    // Three helpers paired with §6.3.6 two-phase flow: validate-only
    // first (no consume), build + sign the op, then CAS-style
    // consume. Pattern follows existing per-purpose email-token
    // helpers (chainlink #62 Case B confirmation).
    // ============================================================

    /// Generate a `plc_operation` email token. TTL = 30 minutes per
    /// §6.3.6 (matches bsky-PDS pattern). Single-use; cleaned up at
    /// consume time via the CAS UPDATE in [`consume_plc_operation_token`].
    pub async fn generate_plc_operation_token(&self, did: &str) -> PdsResult<String> {
        let token = Uuid::new_v4().to_string();
        let now = Utc::now();
        let expires_at = now + Duration::minutes(30);

        sqlx::query(
            r#"
            INSERT INTO email_token (token, did, purpose, created_at, expires_at, used)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(&token)
        .bind(did)
        .bind("plc_operation")
        .bind(now.to_rfc3339())
        .bind(expires_at.to_rfc3339())
        .bind(false)
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;

        Ok(token)
    }

    /// §6.3.6 two-phase step 2: validate-only, **NO consume**. On
    /// success the token row remains untouched so a transient
    /// failure between validate and consume (e.g., transient PLC
    /// directory outage at step 3) leaves the token intact for
    /// retry.
    ///
    /// Fails with `PdsError::Authentication("InvalidToken: ...")` on
    /// missing token, mismatched DID, already-used, or expired. The
    /// `InvalidToken` prefix in the message lets the handler map
    /// uniformly to HTTP 400 `InvalidToken` without dispatching on
    /// the inner cause (cause is logged at debug for observability).
    pub async fn validate_plc_operation_token(
        &self,
        did: &str,
        token: &str,
    ) -> PdsResult<()> {
        let now = Utc::now();
        let row = sqlx::query(
            r#"
            SELECT token, did, purpose, expires_at, used
            FROM email_token
            WHERE token = $1 AND purpose = 'plc_operation'
            "#,
        )
        .bind(token)
        .fetch_optional(&self.db)
        .await
        .map_err(PdsError::Database)?
        .ok_or_else(|| {
            PdsError::Authentication("InvalidToken: no matching plc_operation token".to_string())
        })?;

        let token_did: String = row.try_get("did")?;
        let expires_at: DateTime<Utc> = parse_timestamp(&row.try_get::<String, _>("expires_at")?)?;
        // chainlink #71 — must use crate::db::read_bool, NOT
        // row.try_get::<bool, _>. SQLite stores BOOLEAN as
        // BIGINT 0/1 and sqlx::Any's try_get<bool> errors with
        // "mismatched types; Rust type 'bool' is not compatible
        // with SQL type 'BIGINT'". read_bool dispatches on the
        // backend. Pre-#71 this surfaced as a generic HTTP 500
        // in sign_plc_operation since the handler propagated
        // PdsError::Database without observable cause.
        let used: bool = crate::db::read_bool(&row, "used")?;

        if token_did != did {
            return Err(PdsError::Authentication(
                "InvalidToken: token DID does not match authenticated user".to_string(),
            ));
        }
        if used {
            return Err(PdsError::Authentication(
                "InvalidToken: plc_operation token has already been used".to_string(),
            ));
        }
        if now > expires_at {
            return Err(PdsError::Authentication(
                "InvalidToken: plc_operation token has expired".to_string(),
            ));
        }

        Ok(())
    }

    /// §6.3.6 two-phase step 7: CAS-style consume. Single atomic
    /// UPDATE flips `used: false → true` and reports whether the
    /// row was the one to make the transition. Two simultaneous
    /// calls with the same token race: exactly one returns
    /// `Consumed`; the other returns `AlreadyConsumed`. No double-
    /// consume is possible.
    ///
    /// Per round-4 F2 closure (§6.3.6 enum semantics): CAS UPDATE
    /// returns zero affected rows for BOTH `AlreadyConsumed` AND
    /// `NotFound`. We do a follow-up SELECT for logging
    /// distinguishability only — both map to `TokenAlreadyConsumed`
    /// (HTTP 409) on the wire. In practice `NotFound` is impossible
    /// if [`validate_plc_operation_token`] succeeded immediately
    /// before, but distinguishing logs help debug if the assumption
    /// is ever violated.
    pub async fn consume_plc_operation_token(
        &self,
        did: &str,
        token: &str,
    ) -> ConsumeResult {
        let result = sqlx::query(
            r#"
            UPDATE email_token
            SET used = true
            WHERE token = $1
              AND did = $2
              AND purpose = 'plc_operation'
              AND used = false
            "#,
        )
        .bind(token)
        .bind(did)
        .execute(&self.db)
        .await;

        let exec = match result {
            Ok(e) => e,
            Err(e) => return ConsumeResult::Error(PdsError::Database(e)),
        };

        if exec.rows_affected() >= 1 {
            return ConsumeResult::Consumed;
        }

        // Disambiguate AlreadyConsumed vs NotFound for logging.
        let probe = sqlx::query("SELECT used FROM email_token WHERE token = $1 AND purpose = 'plc_operation'")
            .bind(token)
            .fetch_optional(&self.db)
            .await;
        match probe {
            Ok(Some(_)) => ConsumeResult::AlreadyConsumed,
            Ok(None) => ConsumeResult::NotFound,
            Err(e) => ConsumeResult::Error(PdsError::Database(e)),
        }
    }

    /// Generate account deletion token
    ///
    /// Creates a deletion confirmation token that expires in 1 hour.
    /// This token is sent via email and must be provided to complete deletion.
    pub async fn generate_account_delete_token(&self, did: &str) -> PdsResult<String> {
        let token = Uuid::new_v4().to_string();
        let now = Utc::now();
        let expires_at = now + Duration::hours(1); // Deletion tokens expire in 1 hour

        sqlx::query(
            r#"
            INSERT INTO email_token (token, did, purpose, created_at, expires_at, used)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(&token)
        .bind(did)
        .bind("delete_account")
        .bind(now.to_rfc3339())
        .bind(expires_at.to_rfc3339())
        .bind(false)
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;

        Ok(token)
    }

    /// Validate account deletion token
    ///
    /// Checks if the provided token is valid for account deletion.
    /// Returns the DID associated with the token if valid.
    /// Does NOT mark the token as used - that happens during actual deletion.
    pub async fn validate_account_delete_token(&self, did: &str, token: &str) -> PdsResult<()> {
        let now = Utc::now();

        // Get token info
        let row = sqlx::query(
            r#"
            SELECT token, did, purpose, expires_at, used
            FROM email_token
            WHERE token = $1 AND purpose = 'delete_account'
            "#,
        )
        .bind(token)
        .fetch_optional(&self.db)
        .await
        .map_err(PdsError::Database)?
        .ok_or_else(|| PdsError::Validation("Invalid deletion token".to_string()))?;

        let token_did: String = row.try_get("did").map_err(|e| {
            tracing::error!(
                at_step = "validate_account_delete_token:read_did",
                error = %e,
                "deleteAccount validator: failed to read 'did' column"
            );
            PdsError::Database(e)
        })?;
        let expires_at_str: String = row.try_get("expires_at").map_err(|e| {
            tracing::error!(
                at_step = "validate_account_delete_token:read_expires_at",
                error = %e,
                "deleteAccount validator: failed to read 'expires_at' column"
            );
            PdsError::Database(e)
        })?;
        let expires_at: DateTime<Utc> = parse_timestamp(&expires_at_str).map_err(|e| {
            tracing::error!(
                at_step = "validate_account_delete_token:parse_expires_at",
                error = %e,
                "deleteAccount validator: failed to parse 'expires_at' timestamp"
            );
            e
        })?;
        // chainlink #86 / #74: sqlx::Any does not auto-coerce bool ↔
        // SQLite BIGINT; route through crate::db::read_bool. Same fix
        // pattern as #71 for validate_plc_operation_token.
        let used: bool = crate::db::read_bool(&row, "used").map_err(|e| {
            tracing::error!(
                at_step = "validate_account_delete_token:read_used",
                error = %e,
                "deleteAccount validator: failed to read 'used' column"
            );
            e
        })?;

        // Verify token is for the correct DID
        if token_did != did {
            return Err(PdsError::Validation(
                "Token does not match account".to_string(),
            ));
        }

        // Check if already used
        if used {
            return Err(PdsError::Validation(
                "Deletion token has already been used".to_string(),
            ));
        }

        // Check expiration
        if now > expires_at {
            return Err(PdsError::Validation(
                "Deletion token has expired".to_string(),
            ));
        }

        Ok(())
    }

    /// Mark account deletion token as used
    ///
    /// Called after successful account deletion to prevent token reuse.
    pub async fn mark_delete_token_used(&self, token: &str) -> PdsResult<()> {
        sqlx::query("UPDATE email_token SET used = true WHERE token = $1")
            .bind(token)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

        Ok(())
    }

    /// Generate email update token
    ///
    /// Creates an email update confirmation token that expires in 1 hour.
    /// This token is sent to the current email and must be provided to complete email update.
    pub async fn generate_email_update_token(&self, did: &str) -> PdsResult<String> {
        let token = Uuid::new_v4().to_string();
        let now = Utc::now();
        let expires_at = now + Duration::hours(1);

        sqlx::query(
            r#"
            INSERT INTO email_token (token, did, purpose, created_at, expires_at, used)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(&token)
        .bind(did)
        .bind("update_email")
        .bind(now.to_rfc3339())
        .bind(expires_at.to_rfc3339())
        .bind(false)
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;

        Ok(token)
    }

    /// Validate email update token
    ///
    /// Checks if the provided token is valid for email update.
    /// Marks the token as used upon successful validation.
    pub async fn validate_email_update_token(&self, did: &str, token: &str) -> PdsResult<()> {
        let now = Utc::now();

        let row = sqlx::query(
            r#"
            SELECT token, did, purpose, expires_at, used
            FROM email_token
            WHERE token = $1 AND purpose = 'update_email'
            "#,
        )
        .bind(token)
        .fetch_optional(&self.db)
        .await
        .map_err(PdsError::Database)?
        .ok_or_else(|| PdsError::Validation("Invalid email update token".to_string()))?;

        let token_did: String = row.try_get("did")?;
        let expires_at: DateTime<Utc> = parse_timestamp(&row.try_get::<String, _>("expires_at")?)?;
        // chainlink #74 / #86: sqlx::Any bool/BIGINT mismatch fix.
        let used: bool = crate::db::read_bool(&row, "used")?;

        if token_did != did {
            return Err(PdsError::Validation(
                "Token does not match account".to_string(),
            ));
        }

        if used {
            return Err(PdsError::Validation(
                "Email update token has already been used".to_string(),
            ));
        }

        if now > expires_at {
            return Err(PdsError::Validation(
                "Email update token has expired".to_string(),
            ));
        }

        // Mark token as used
        sqlx::query("UPDATE email_token SET used = true WHERE token = $1")
            .bind(token)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

        Ok(())
    }

    /// Update account email address
    ///
    /// Updates the email address for an account.
    /// Returns an error if the email is already in use by another account.
    pub async fn update_email(&self, did: &str, new_email: &str) -> PdsResult<()> {
        let mut tx = self.db.begin().await.map_err(PdsError::Database)?;
        Self::update_email_in_tx(&mut tx, did, new_email).await?;
        tx.commit().await.map_err(PdsError::Database)?;
        Ok(())
    }

    /// Update account email address inside an existing transaction.
    /// LB-1 / chainlink #128 atomic-with-chain entry point.
    ///
    /// Scope: this `_in_tx` variant covers only the primary
    /// `account` table mutation (and the in-tx uniqueness check that
    /// guards it). Multi-store side effects associated with the
    /// email change — e.g., invalidating outstanding email-update
    /// tokens, queueing a confirmation email — remain outside the
    /// transaction with their existing post-commit best-effort
    /// handling. Per design doc §3.4 the chain-of-custody invariant
    /// is "chain entry atomic with the underlying mutation"; that
    /// underlying mutation is the `account` row update. The
    /// multi-store cleanup question is a separate concern from the
    /// LB-1 atomicity guarantee.
    pub async fn update_email_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        did: &str,
        new_email: &str,
    ) -> PdsResult<()> {
        // Check if email is already in use by another account.
        // Inside the tx so the check + UPDATE see one snapshot —
        // otherwise two concurrent updates could both pass the
        // uniqueness check.
        let existing = sqlx::query("SELECT did FROM account WHERE email = $1 AND did != $2")
            .bind(new_email)
            .bind(did)
            .fetch_optional(&mut **tx)
            .await
            .map_err(PdsError::Database)?;

        if existing.is_some() {
            return Err(PdsError::Validation(
                "This email address is already in use".to_string(),
            ));
        }

        // Update email and clear email confirmation
        sqlx::query("UPDATE account SET email = $1, email_confirmed_at = NULL WHERE did = $2")
            .bind(new_email)
            .bind(did)
            .execute(&mut **tx)
            .await
            .map_err(PdsError::Database)?;

        tracing::info!(
            did = %did,
            new_email = %new_email,
            "account_email_updated"
        );

        Ok(())
    }

    /// Update password inside an existing transaction. LB-1 / chainlink #129
    /// atomic-with-chain entry point. Performs the hash before opening
    /// the tx so the (slow) Argon2 work doesn't extend the transaction's
    /// lifetime.
    pub async fn update_password_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        did: &str,
        new_password: &str,
    ) -> PdsResult<()> {
        let password_hash = crate::auth::PasswordHasher::hash(new_password)
            .map_err(|e| PdsError::Internal(format!("Password hashing failed: {}", e)))?;
        Self::update_password_hash_in_tx(tx, did, &password_hash).await
    }

    /// Apply the password UPDATE + session/refresh_token DELETE inside
    /// the caller's transaction. Used by both pool-API and `_in_tx`
    /// variants.
    async fn update_password_hash_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        did: &str,
        password_hash: &str,
    ) -> PdsResult<()> {
        let result = sqlx::query("UPDATE account SET password_hash = $1 WHERE did = $2")
            .bind(password_hash)
            .bind(did)
            .execute(&mut **tx)
            .await
            .map_err(PdsError::Database)?;

        if result.rows_affected() == 0 {
            return Err(PdsError::NotFound(format!("Account not found: {}", did)));
        }

        // Invalidate all sessions for this account (security best practice)
        sqlx::query("DELETE FROM session WHERE did = $1")
            .bind(did)
            .execute(&mut **tx)
            .await
            .map_err(PdsError::Database)?;

        // Also delete all refresh tokens
        sqlx::query("DELETE FROM refresh_token WHERE did = $1")
            .bind(did)
            .execute(&mut **tx)
            .await
            .map_err(PdsError::Database)?;

        tracing::info!(
            did = %did,
            "account_password_updated_by_admin"
        );

        Ok(())
    }

    /// Delete account permanently
    ///
    /// Permanently removes the account from the database.
    /// This should only be called after token validation.
    pub async fn delete_account_permanent(&self, did: &str) -> PdsResult<()> {
        let mut tx = self.db.begin().await.map_err(PdsError::Database)?;
        Self::delete_account_permanent_in_tx(&mut tx, did).await?;
        tx.commit().await.map_err(PdsError::Database)?;
        Ok(())
    }

    /// Permanently delete an account inside an existing transaction.
    /// LB-1 / chainlink #129 atomic-with-chain entry point.
    pub async fn delete_account_permanent_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        did: &str,
    ) -> PdsResult<()> {
        // Delete from all related tables
        for table in [
            "session",
            "refresh_token",
            "app_password",
            "email_token",
            "plc_keys",
            "account",
            "actor",
        ] {
            let sql = format!("DELETE FROM {} WHERE did = $1", table);
            sqlx::query(&sql)
                .bind(did)
                .execute(&mut **tx)
                .await
                .map_err(PdsError::Database)?;
        }

        tracing::info!("Account permanently deleted: DID={}", did);

        Ok(())
    }

    /// Check if account is marked for deletion
    #[allow(dead_code)] // Future account deletion feature
    pub async fn is_account_pending_deletion(&self, did: &str) -> PdsResult<bool> {
        let row = sqlx::query("SELECT deactivated_at FROM actor WHERE did = $1")
            .bind(did)
            .fetch_optional(&self.db)
            .await
            .map_err(PdsError::Database)?
            .ok_or_else(|| PdsError::NotFound("Actor not found".to_string()))?;

        let deactivated_at_s: Option<String> = row.try_get("deactivated_at")?;
        Ok(deactivated_at_s.is_some())
    }

    /// Cancel account deletion (if within grace period)
    pub async fn cancel_account_deletion(&self, did: &str) -> PdsResult<()> {
        sqlx::query("UPDATE actor SET deactivated_at = NULL, delete_after = NULL WHERE did = $1")
            .bind(did)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

        tracing::info!("Account deletion cancelled for DID: {}", did);

        Ok(())
    }

    /// Deactivate an account (temporary suspension)
    ///
    /// Sets deactivated_at to NOW without setting delete_after.
    /// This allows users to temporarily disable their account without initiating deletion.
    /// The account can be reactivated anytime via reactivate_account() or login.
    ///
    /// Differences from deletion:
    /// - Deactivation: Temporary, reversible anytime (deactivated_at set, delete_after NULL)
    /// - Deletion: Permanent with grace period (delete_after set to 30 days in future)
    ///
    /// # Arguments
    /// * `did` - The DID of the account to deactivate
    pub async fn deactivate_account(&self, did: &str) -> PdsResult<()> {
        let mut tx = self.db.begin().await.map_err(PdsError::Database)?;
        Self::deactivate_account_in_tx(&mut tx, did).await?;
        tx.commit().await.map_err(PdsError::Database)?;
        Ok(())
    }

    /// Deactivate an account inside an existing transaction. LB-1 /
    /// chainlink #129 atomic-with-chain entry point.
    pub async fn deactivate_account_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        did: &str,
    ) -> PdsResult<()> {
        let now = Utc::now();
        sqlx::query("UPDATE actor SET deactivated_at = $1, delete_after = NULL WHERE did = $2")
            .bind(now.to_rfc3339())
            .bind(did)
            .execute(&mut **tx)
            .await
            .map_err(PdsError::Database)?;
        sqlx::query("DELETE FROM session WHERE did = $1")
            .bind(did)
            .execute(&mut **tx)
            .await
            .map_err(PdsError::Database)?;
        sqlx::query("DELETE FROM refresh_token WHERE did = $1")
            .bind(did)
            .execute(&mut **tx)
            .await
            .map_err(PdsError::Database)?;
        tracing::info!("Account temporarily deactivated for DID: {}", did);
        Ok(())
    }

    /// Reactivate a deactivated account
    ///
    /// Clears deactivated_at to restore account to active state.
    /// User can then login normally to create new sessions.
    ///
    /// # Arguments
    /// * `did` - The DID of the account to reactivate
    pub async fn reactivate_account(&self, did: &str) -> PdsResult<()> {
        let mut tx = self.db.begin().await.map_err(PdsError::Database)?;
        Self::reactivate_account_in_tx(&mut tx, did).await?;
        tx.commit().await.map_err(PdsError::Database)?;
        Ok(())
    }

    /// Reactivate an account inside an existing transaction. LB-1 /
    /// chainlink #129 atomic-with-chain entry point.
    pub async fn reactivate_account_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        did: &str,
    ) -> PdsResult<()> {
        sqlx::query("UPDATE actor SET deactivated_at = NULL WHERE did = $1")
            .bind(did)
            .execute(&mut **tx)
            .await
            .map_err(PdsError::Database)?;
        tracing::info!("Account reactivated for DID: {}", did);
        Ok(())
    }

    /// Takedown an account (remove from public view). Begins its own
    /// transaction; production handlers should prefer
    /// `takedown_account_in_tx` so the chain-entry write rides the
    /// caller's transaction. Consumed by `#[cfg(test)]` sites in
    /// `src/api/admin.rs` for handler-level coverage.
    #[allow(dead_code)]
    pub async fn takedown_account(&self, did: &str, takedown_ref: &str) -> PdsResult<()> {
        let mut tx = self.db.begin().await.map_err(PdsError::Database)?;
        Self::takedown_account_in_tx(&mut tx, did, takedown_ref).await?;
        tx.commit().await.map_err(PdsError::Database)?;
        Ok(())
    }

    /// Takedown an account inside an existing transaction. LB-1 /
    /// chainlink #128 atomic-with-chain entry point. Performs the
    /// same three writes as the pool-API wrapper — actor takedown_ref
    /// UPDATE + session DELETE + refresh_token DELETE — against the
    /// caller-supplied transaction. Caller commits.
    pub async fn takedown_account_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        did: &str,
        takedown_ref: &str,
    ) -> PdsResult<()> {
        // Set takedown_ref in actor table
        let result = sqlx::query("UPDATE actor SET takedown_ref = $1 WHERE did = $2")
            .bind(takedown_ref)
            .bind(did)
            .execute(&mut **tx)
            .await
            .map_err(PdsError::Database)?;

        if result.rows_affected() == 0 {
            return Err(PdsError::NotFound(format!("Actor {} not found", did)));
        }

        // Delete all active sessions for this account
        sqlx::query("DELETE FROM session WHERE did = $1")
            .bind(did)
            .execute(&mut **tx)
            .await
            .map_err(PdsError::Database)?;

        // Delete all refresh tokens for this account
        sqlx::query("DELETE FROM refresh_token WHERE did = $1")
            .bind(did)
            .execute(&mut **tx)
            .await
            .map_err(PdsError::Database)?;

        tracing::info!(
            "Account taken down: DID={}, takedown_ref={}, sessions and tokens revoked",
            did,
            takedown_ref
        );

        Ok(())
    }

    /// Activate an account inside an existing transaction (clear
    /// takedown_ref). LB-1 / chainlink #129 atomic-with-chain entry point.
    pub async fn activate_account_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        did: &str,
    ) -> PdsResult<()> {
        let result = sqlx::query("UPDATE actor SET takedown_ref = NULL WHERE did = $1")
            .bind(did)
            .execute(&mut **tx)
            .await
            .map_err(PdsError::Database)?;
        if result.rows_affected() == 0 {
            return Err(PdsError::NotFound(format!("Actor {} not found", did)));
        }
        tracing::info!("Account activated for DID: {}", did);
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
            return Err(PdsError::Validation(
                "App password name cannot be empty".to_string(),
            ));
        }

        if name.len() > 100 {
            return Err(PdsError::Validation(
                "App password name too long".to_string(),
            ));
        }

        // Check if app password with this name already exists for this user
        let existing = sqlx::query("SELECT name FROM app_password WHERE did = $1 AND name = $2")
            .bind(did)
            .bind(name)
            .fetch_optional(&self.db)
            .await
            .map_err(PdsError::Database)?;

        if existing.is_some() {
            return Err(PdsError::Conflict(format!(
                "App password '{}' already exists",
                name
            )));
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
        let password_hash = crate::auth::PasswordHasher::hash(&raw_password)
            .map_err(|e| PdsError::Internal(format!("Password hashing failed: {}", e)))?;

        // Store app password
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO app_password (did, name, password_hash, created_at, privileged)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(did)
        .bind(name)
        .bind(&password_hash)
        .bind(now.to_rfc3339())
        .bind(privileged)
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;

        tracing::info!("Created app password '{}' for DID: {}", name, did);

        // Return the raw password (only time it's shown to user)
        Ok(raw_password)
    }

    /// List all app passwords for a user (without the actual passwords)
    pub async fn list_app_passwords(&self, did: &str) -> PdsResult<Vec<AppPasswordInfo>> {
        let rows = sqlx::query(
            "SELECT name, created_at, privileged FROM app_password WHERE did = $1 ORDER BY created_at DESC"
        )
        .bind(did)
        .fetch_all(&self.db)
        .await
        .map_err(PdsError::Database)?;

        let mut passwords = Vec::new();
        for row in rows {
            passwords.push(AppPasswordInfo {
                name: row.get("name"),
                created_at: parse_timestamp(&row.get::<String, _>("created_at"))?,
                privileged: crate::db::read_bool(&row, "privileged")?,
            });
        }

        Ok(passwords)
    }

    /// Revoke (delete) an app password
    pub async fn revoke_app_password(&self, did: &str, name: &str) -> PdsResult<()> {
        let result = sqlx::query("DELETE FROM app_password WHERE did = $1 AND name = $2")
            .bind(did)
            .bind(name)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

        if result.rows_affected() == 0 {
            return Err(PdsError::NotFound(format!(
                "App password '{}' not found",
                name
            )));
        }

        // Delete all sessions created with this app password
        sqlx::query("DELETE FROM session WHERE did = $1 AND app_password_name = $2")
            .bind(did)
            .bind(name)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

        tracing::info!("Revoked app password '{}' for DID: {}", name, did);

        Ok(())
    }

    /// Authenticate with app password
    ///
    /// This function includes timing attack protection by ensuring a minimum
    /// execution time of 350ms to prevent username enumeration via timing analysis.
    pub async fn login_with_app_password(
        &self,
        identifier: &str,
        app_password: &str,
    ) -> PdsResult<(ActorAccount, Session, String)> {
        // Start timing for attack mitigation
        let start = std::time::Instant::now();

        // Perform login logic - wrap in a scope so we can handle timing in finally block
        let result = async {
            // Find account
            let account = self.get_account_by_identifier(identifier).await?;

            // Check if account is deactivated or taken down
            if account.deactivated_at.is_some() {
                return Err(PdsError::Authorization(
                    "Account is deactivated".to_string(),
                ));
            }

            if account.takedown_ref.is_some() {
                return Err(PdsError::Authorization(
                    "Account has been taken down".to_string(),
                ));
            }

            // Find matching app password by trying to verify against all user's app passwords
            let rows = sqlx::query("SELECT name, password_hash FROM app_password WHERE did = $1")
                .bind(&account.did)
                .fetch_all(&self.db)
                .await
                .map_err(PdsError::Database)?;

            let mut matched_name: Option<String> = None;
            for row in rows {
                let name: String = row.get("name");
                let hash: String = row.get("password_hash");

                if let Ok(true) = crate::auth::PasswordHasher::verify(app_password, &hash) {
                    matched_name = Some(name);
                    break;
                }
            }

            let app_password_name = matched_name
                .ok_or_else(|| PdsError::Authentication("Invalid app password".to_string()))?;

            // Create session with app_password_name
            let session = self
                .create_session(&account.did, Some(app_password_name.clone()))
                .await?;

            Ok((account, session, app_password_name))
        }
        .await;

        // Mitigate timing attacks by ensuring minimum execution time
        // This prevents attackers from distinguishing valid vs invalid usernames
        // based on response time differences
        let elapsed = start.elapsed().as_millis() as i64;
        let wait_time = 350 - elapsed;
        if wait_time > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(wait_time as u64)).await;
        }

        result
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

    /// Validate handle format using comprehensive ATProto validation
    fn validate_handle(&self, handle: &str) -> PdsResult<()> {
        // Use comprehensive validation from identity module
        crate::identity::validate_handle(handle, &self.config.identity.service_handle_domains)?;
        Ok(())
    }

    /// Validate email format
    fn validate_email(&self, email: &str) -> PdsResult<()> {
        // v0.8 arc 3 (#184) — reject ':' in the email. Keeps the login
        // resolver's DID branch a clean DID-only lookup: a 'did:'-leading
        // email can no longer be created, so a 'did:'-prefixed login
        // identifier is unambiguously a DID. General charset rule, not a
        // did:-special-case (a bare ':' outside a quoted local-part is
        // non-RFC5321-compliant in any case).
        if email.contains(':') {
            return Err(PdsError::Validation(
                "Email address must not contain ':'".to_string(),
            ));
        }

        // Basic email validation
        if !email.contains('@') {
            return Err(PdsError::Validation("Invalid email format".to_string()));
        }

        Ok(())
    }

    // ==================== Invite Code System ====================

    /// Create an invite code
    ///
    /// # Arguments
    /// * `created_by` - DID of the user creating the invite
    /// * `use_count` - Number of times this invite can be used (default: 1)
    /// * `for_account` - Optional DID if this invite is for a specific person
    ///
    /// # Returns
    /// * The generated invite code string
    pub async fn create_invite_code(
        &self,
        created_by: &str,
        use_count: i32,
        for_account: Option<String>,
    ) -> PdsResult<String> {
        // Generate a random invite code (format: xxxx-xxxx-xxxx-xxxx)
        let code = format!(
            "{}-{}-{}-{}",
            Self::generate_random_string(4),
            Self::generate_random_string(4),
            Self::generate_random_string(4),
            Self::generate_random_string(4)
        );

        let now = Utc::now();

        sqlx::query(
            "INSERT INTO invite_code (code, available_uses, disabled, created_by, created_at, created_for)
             VALUES ($1, $2, $3, $4, $5, $6)"
        )
        .bind(&code)
        .bind(use_count)
        .bind(false)
        .bind(created_by)
        .bind(now.to_rfc3339())
        .bind(&for_account)
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;

        tracing::info!(
            "Created invite code {} by {} (uses: {}, for: {:?})",
            code,
            created_by,
            use_count,
            for_account
        );

        Ok(code)
    }

    /// Validate an invite code and return information about it
    ///
    /// Checks if the code exists, is not disabled, and has available uses.
    pub async fn validate_invite_code(&self, code: &str, used_by: Option<&str>) -> PdsResult<()> {
        // Check if invites are required
        if !self.config.invites.required {
            // Invites not required, always succeed
            return Ok(());
        }

        let row = sqlx::query(
            "SELECT code, available_uses, disabled, created_for FROM invite_code WHERE code = $1",
        )
        .bind(code)
        .fetch_optional(&self.db)
        .await
        .map_err(PdsError::Database)?
        .ok_or_else(|| PdsError::Validation("Invalid invite code".to_string()))?;

        let available_uses: i32 = row.get("available_uses");
        let disabled: bool = crate::db::read_bool(&row, "disabled")?;
        let created_for: Option<String> = row.get("created_for");

        // Check if disabled
        if disabled {
            return Err(PdsError::Validation(
                "Invite code has been disabled".to_string(),
            ));
        }

        // Check if uses remain
        if available_uses <= 0 {
            return Err(PdsError::Validation(
                "Invite code has no uses remaining".to_string(),
            ));
        }

        // Check if code is for a specific person
        if let Some(specific_did) = created_for {
            if let Some(user_did) = used_by {
                if user_did != specific_did {
                    return Err(PdsError::Validation(
                        "Invite code is reserved for another user".to_string(),
                    ));
                }
            }
        }

        Ok(())
    }

    /// Use an invite code (called during account creation)
    ///
    /// Decrements available uses and records usage.
    pub async fn use_invite_code(&self, code: &str, used_by: &str) -> PdsResult<()> {
        // Check if invites are required
        if !self.config.invites.required {
            // Invites not required, no-op
            return Ok(());
        }

        let now = Utc::now();

        // Begin transaction
        let mut tx = self.db.begin().await.map_err(PdsError::Database)?;

        // Validate code
        let row =
            sqlx::query("SELECT code, available_uses, disabled FROM invite_code WHERE code = $1")
                .bind(code)
                .fetch_optional(&mut *tx)
                .await
                .map_err(PdsError::Database)?
                .ok_or_else(|| PdsError::Validation("Invalid invite code".to_string()))?;

        let available_uses: i32 = row.get("available_uses");
        let disabled: bool = crate::db::read_bool(&row, "disabled")?;

        if disabled {
            return Err(PdsError::Validation(
                "Invite code has been disabled".to_string(),
            ));
        }

        if available_uses <= 0 {
            return Err(PdsError::Validation(
                "Invite code has no uses remaining".to_string(),
            ));
        }

        // Record usage
        sqlx::query(
            "INSERT INTO invite_code_use (code, used_by, used_at)
             VALUES ($1, $2, $3)",
        )
        .bind(code)
        .bind(used_by)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(PdsError::Database)?;

        // Decrement available uses
        sqlx::query("UPDATE invite_code SET available_uses = available_uses - 1 WHERE code = $1")
            .bind(code)
            .execute(&mut *tx)
            .await
            .map_err(PdsError::Database)?;

        tx.commit().await.map_err(PdsError::Database)?;

        tracing::info!("Invite code {} used by {}", code, used_by);

        Ok(())
    }

    /// List invite codes created by a user
    pub async fn list_invite_codes(
        &self,
        created_by: &str,
    ) -> PdsResult<Vec<crate::db::account::InviteCode>> {
        // Manual row → struct conversion: the auto-derived FromRow on
        // InviteCode wants `Decode<Any>` for chrono::DateTime, which
        // sqlx::Any doesn't provide. We read created_at as String and
        // parse via parse_timestamp.
        let rows = sqlx::query(
            "SELECT code, available_uses, disabled, created_by, created_at, created_for
             FROM invite_code
             WHERE created_by = $1
             ORDER BY created_at DESC",
        )
        .bind(created_by)
        .fetch_all(&self.db)
        .await
        .map_err(PdsError::Database)?;

        rows.into_iter()
            .map(|row| {
                let created_at_s: String = row.try_get("created_at")?;
                Ok(crate::db::account::InviteCode {
                    code: row.try_get("code")?,
                    available_uses: row.try_get("available_uses")?,
                    disabled: crate::db::read_bool(&row, "disabled")?,
                    created_by: row.try_get("created_by")?,
                    created_at: parse_timestamp(&created_at_s)?,
                    created_for: row.try_get("created_for")?,
                })
            })
            .collect()
    }

    /// Get usage history for an invite code
    pub async fn get_invite_code_usage(
        &self,
        code: &str,
    ) -> PdsResult<Vec<crate::db::account::InviteCodeUse>> {
        let rows = sqlx::query(
            "SELECT code, used_by, used_at
             FROM invite_code_use
             WHERE code = $1
             ORDER BY used_at DESC",
        )
        .bind(code)
        .fetch_all(&self.db)
        .await
        .map_err(PdsError::Database)?;

        rows.into_iter()
            .map(|row| {
                let used_at_s: String = row.try_get("used_at")?;
                Ok(crate::db::account::InviteCodeUse {
                    code: row.try_get("code")?,
                    used_by: row.try_get("used_by")?,
                    used_at: parse_timestamp(&used_at_s)?,
                })
            })
            .collect()
    }

    /// Disable an invite code (admin/creator only)
    #[allow(dead_code)] // Future invite management feature
    pub async fn disable_invite_code(&self, code: &str, requesting_did: &str) -> PdsResult<()> {
        // Verify requester is the creator or an admin
        let row = sqlx::query("SELECT created_by FROM invite_code WHERE code = $1")
            .bind(code)
            .fetch_optional(&self.db)
            .await
            .map_err(PdsError::Database)?
            .ok_or_else(|| PdsError::NotFound("Invite code not found".to_string()))?;

        let created_by: String = row.get("created_by");

        // Creator-only at this layer. Admin-flavored invite-disable
        // (operator disabling another account's code) belongs at the
        // admin XRPC handler tier where AdminAuthContext gates entry
        // — see src/api/admin.rs::disable_invite_code, which routes
        // through invite_manager and never calls this method.
        if created_by != requesting_did {
            return Err(PdsError::Authorization(
                "Only the creator can disable this invite code at the account layer; \
                 admin-flavored disable goes through tools.aurora.* / com.atproto.admin.*"
                    .to_string(),
            ));
        }

        // Disable the code
        sqlx::query("UPDATE invite_code SET disabled = TRUE WHERE code = $1")
            .bind(code)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

        tracing::info!("Invite code {} disabled by {}", code, requesting_did);

        Ok(())
    }

    /// Enable invite code creation for an account inside an existing
    /// transaction. LB-1 / chainlink #122 atomic-with-chain entry point.
    pub async fn enable_account_invites_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        did: &str,
    ) -> PdsResult<()> {
        let result = sqlx::query("UPDATE account SET invites_disabled = FALSE WHERE did = $1")
            .bind(did)
            .execute(&mut **tx)
            .await
            .map_err(PdsError::Database)?;

        if result.rows_affected() == 0 {
            return Err(PdsError::NotFound(format!("Account not found: {}", did)));
        }

        tracing::info!(did = %did, "account_invites_enabled");
        Ok(())
    }

    /// Disable invite code creation for an account. Begins its own
    /// transaction; production handlers should prefer
    /// `disable_account_invites_in_tx` so the chain-entry write rides
    /// the caller's transaction. Consumed by `#[cfg(test)]` sites in
    /// `src/api/admin.rs` for handler-level coverage.
    #[allow(dead_code)]
    pub async fn disable_account_invites(&self, did: &str) -> PdsResult<()> {
        let mut tx = self.db.begin().await.map_err(PdsError::Database)?;
        Self::disable_account_invites_in_tx(&mut tx, did).await?;
        tx.commit().await.map_err(PdsError::Database)?;
        Ok(())
    }

    /// Disable invite code creation for an account inside an existing
    /// transaction. LB-1 / chainlink #122 atomic-with-chain entry point.
    pub async fn disable_account_invites_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        did: &str,
    ) -> PdsResult<()> {
        let result = sqlx::query("UPDATE account SET invites_disabled = TRUE WHERE did = $1")
            .bind(did)
            .execute(&mut **tx)
            .await
            .map_err(PdsError::Database)?;

        if result.rows_affected() == 0 {
            return Err(PdsError::NotFound(format!("Account not found: {}", did)));
        }

        tracing::info!(did = %did, "account_invites_disabled");
        Ok(())
    }

    #[allow(dead_code)] // Future invite allocation feature
    pub async fn allocate_invite_codes(&self, did: &str, count: i32) -> PdsResult<Vec<String>> {
        // Check if invites are disabled for this account
        let row = sqlx::query("SELECT invites_disabled FROM account WHERE did = $1")
            .bind(did)
            .fetch_optional(&self.db)
            .await
            .map_err(PdsError::Database)?
            .ok_or_else(|| PdsError::NotFound("Account not found".to_string()))?;

        let invites_disabled: bool = crate::db::read_bool(&row, "invites_disabled")?;

        if invites_disabled {
            return Ok(Vec::new()); // Don't allocate if disabled
        }

        // Create invite codes
        let mut codes = Vec::new();
        for _ in 0..count {
            let code = self.create_invite_code(did, 1, None).await?;
            codes.push(code);
        }

        Ok(codes)
    }

    /// List all accounts with pagination
    ///
    /// Returns accounts ordered by DID for consistent pagination.
    /// Use the last DID as cursor for next page.
    /// Joins actor and account tables to get complete information.
    /// Search accounts by email (case-insensitive exact match) with cursor
    /// pagination ordered by `did`.
    ///
    /// Cursor opaqueness: the cursor value is the last `did` returned in the
    /// previous page; callers should treat it as a black box. When
    /// `email` is `None`, returns all accounts (matching the behavior
    /// `searchAccounts` exposes when called without an email parameter).
    pub async fn search_accounts(
        &self,
        email: Option<&str>,
        cursor: Option<&str>,
        limit: i64,
    ) -> PdsResult<Vec<ActorAccount>> {
        // Build SQL with optional email and cursor predicates so we don't run
        // a join+filter when the caller only wants pagination.
        let mut sql = String::from(
            "SELECT
                a.did, a.handle, a.created_at, a.takedown_ref, a.deactivated_at, a.delete_after,
                a.suspended_at, a.desynchronized_at,
                ac.email, ac.password_hash, ac.email_confirmed_at, ac.invites_disabled
             FROM actor a
             LEFT JOIN account ac ON a.did = ac.did
             WHERE 1=1",
        );
        if email.is_some() {
            sql.push_str(" AND LOWER(ac.email) = LOWER(?)");
        }
        if cursor.is_some() {
            sql.push_str(" AND a.did > ?");
        }
        sql.push_str(" ORDER BY a.did LIMIT ?");

        let mut q = sqlx::query(&sql);
        if let Some(e) = email {
            q = q.bind(e);
        }
        if let Some(c) = cursor {
            q = q.bind(c);
        }
        q = q.bind(limit);

        let rows = q.fetch_all(&self.db).await.map_err(PdsError::Database)?;

        let mut accounts = Vec::new();
        for row in rows {
            accounts.push(ActorAccount {
                did: row.get("did"),
                handle: row.get("handle"),
                created_at: parse_timestamp(&row.get::<String, _>("created_at"))?,
                takedown_ref: row.get("takedown_ref"),
                deactivated_at: opt_parse_timestamp(row.get::<Option<String>, _>("deactivated_at"))?,
                delete_after: opt_parse_timestamp(row.get::<Option<String>, _>("delete_after"))?,
                suspended_at: opt_parse_timestamp(row.get::<Option<String>, _>("suspended_at"))?,
                desynchronized_at: opt_parse_timestamp(row.get::<Option<String>, _>("desynchronized_at"))?,
                email: row.get("email"),
                password_hash: row.get("password_hash"),
                email_confirmed_at: opt_parse_timestamp(row.get::<Option<String>, _>("email_confirmed_at"))?,
                invites_disabled: Some(crate::db::read_bool(&row, "invites_disabled")?),
            });
        }

        Ok(accounts)
    }

    pub async fn list_accounts(
        &self,
        cursor: Option<&str>,
        limit: i64,
    ) -> PdsResult<Vec<ActorAccount>> {
        let rows = if let Some(cursor_did) = cursor {
            sqlx::query(
                "SELECT
                    a.did, a.handle, a.created_at, a.takedown_ref, a.deactivated_at, a.delete_after,
                    a.suspended_at, a.desynchronized_at,
                    ac.email, ac.password_hash, ac.email_confirmed_at, ac.invites_disabled
                 FROM actor a
                 LEFT JOIN account ac ON a.did = ac.did
                 WHERE a.did > $1
                 ORDER BY a.did
                 LIMIT $2",
            )
            .bind(cursor_did)
            .bind(limit)
            .fetch_all(&self.db)
            .await
            .map_err(PdsError::Database)?
        } else {
            sqlx::query(
                "SELECT
                    a.did, a.handle, a.created_at, a.takedown_ref, a.deactivated_at, a.delete_after,
                    a.suspended_at, a.desynchronized_at,
                    ac.email, ac.password_hash, ac.email_confirmed_at, ac.invites_disabled
                 FROM actor a
                 LEFT JOIN account ac ON a.did = ac.did
                 ORDER BY a.did
                 LIMIT $1",
            )
            .bind(limit)
            .fetch_all(&self.db)
            .await
            .map_err(PdsError::Database)?
        };

        let mut accounts = Vec::new();
        for row in rows {
            accounts.push(ActorAccount {
                // Actor fields
                did: row.get("did"),
                handle: row.get("handle"),
                created_at: parse_timestamp(&row.get::<String, _>("created_at"))?,
                takedown_ref: row.get("takedown_ref"),
                deactivated_at: opt_parse_timestamp(row.get::<Option<String>, _>("deactivated_at"))?,
                delete_after: opt_parse_timestamp(row.get::<Option<String>, _>("delete_after"))?,
                suspended_at: opt_parse_timestamp(row.get::<Option<String>, _>("suspended_at"))?,
                desynchronized_at: opt_parse_timestamp(row.get::<Option<String>, _>("desynchronized_at"))?,
                // Account fields (may be None for federated actors)
                email: row.get("email"),
                password_hash: row.get("password_hash"),
                email_confirmed_at: opt_parse_timestamp(row.get::<Option<String>, _>("email_confirmed_at"))?,
                invites_disabled: Some(crate::db::read_bool(&row, "invites_disabled")?),
            });
        }

        Ok(accounts)
    }

    /// Operator-facing account listing with broader filters than the
    /// bsky-PDS-flavored `search_accounts` (chainlink #84).
    ///
    /// All filters are AND'd; pagination is the same trailing-DID cursor
    /// scheme used by `search_accounts` and `list_accounts`.
    ///
    /// # Filters
    /// - `signup_from` / `signup_to`: RFC3339 datetime range over
    ///   `actor.created_at` (inclusive on both ends).
    /// - `invite_source`: DID of the account that *created* the invite
    ///   code used to onboard the row. Joins `invite_code_use` →
    ///   `invite_code` and matches `invite_code.created_by`.
    /// - `status`: one of `active` | `deactivated` | `takedown` |
    ///   `suspended`. Caller must validate the value (handler does);
    ///   any other value yields no status filter.
    ///   - `active`: no takedown_ref, no deactivated_at, no active
    ///     non-reversed suspend.
    ///   - `deactivated`: deactivated_at IS NOT NULL.
    ///   - `takedown`: takedown_ref IS NOT NULL.
    ///   - `suspended`: at least one non-reversed `account_moderation`
    ///     row with `action='suspend'` whose `expires_at` is NULL or
    ///     in the future.
    pub async fn ops_list_accounts(
        &self,
        signup_from: Option<&str>,
        signup_to: Option<&str>,
        invite_source: Option<&str>,
        status: Option<&str>,
        cursor: Option<&str>,
        limit: i64,
    ) -> PdsResult<Vec<ActorAccount>> {
        let mut sql = String::from(
            "SELECT \
                a.did, a.handle, a.created_at, a.takedown_ref, a.deactivated_at, a.delete_after, \
                a.suspended_at, a.desynchronized_at, \
                ac.email, ac.password_hash, ac.email_confirmed_at, ac.invites_disabled \
             FROM actor a \
             LEFT JOIN account ac ON a.did = ac.did",
        );

        let mut clauses: Vec<&'static str> = Vec::new();
        let mut bind_strs: Vec<String> = Vec::new();
        let now_iso = Utc::now().to_rfc3339();

        if let Some(d) = invite_source {
            clauses.push(
                "EXISTS (SELECT 1 FROM invite_code_use icu \
                 JOIN invite_code ic ON icu.code = ic.code \
                 WHERE icu.used_by = a.did AND ic.created_by = ?)",
            );
            bind_strs.push(d.to_string());
        }
        if let Some(s) = signup_from {
            clauses.push("a.created_at >= ?");
            bind_strs.push(s.to_string());
        }
        if let Some(s) = signup_to {
            clauses.push("a.created_at <= ?");
            bind_strs.push(s.to_string());
        }
        if let Some(c) = cursor {
            clauses.push("a.did > ?");
            bind_strs.push(c.to_string());
        }
        match status {
            Some("active") => {
                clauses.push("a.takedown_ref IS NULL");
                clauses.push("a.deactivated_at IS NULL");
                clauses.push(
                    "NOT EXISTS (SELECT 1 FROM account_moderation am \
                     WHERE am.did = a.did AND am.action = 'suspend' AND NOT am.reversed \
                       AND (am.expires_at IS NULL OR am.expires_at > ?))",
                );
                bind_strs.push(now_iso.clone());
            }
            Some("deactivated") => {
                clauses.push("a.deactivated_at IS NOT NULL");
            }
            Some("takedown") => {
                clauses.push("a.takedown_ref IS NOT NULL");
            }
            Some("suspended") => {
                clauses.push(
                    "EXISTS (SELECT 1 FROM account_moderation am \
                     WHERE am.did = a.did AND am.action = 'suspend' AND NOT am.reversed \
                       AND (am.expires_at IS NULL OR am.expires_at > ?))",
                );
                bind_strs.push(now_iso.clone());
            }
            _ => {}
        }

        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY a.did LIMIT ?");

        let mut q = sqlx::query(&sql);
        for b in &bind_strs {
            q = q.bind(b);
        }
        q = q.bind(limit);

        let rows = q.fetch_all(&self.db).await.map_err(PdsError::Database)?;

        let mut accounts = Vec::with_capacity(rows.len());
        for row in rows {
            accounts.push(ActorAccount {
                did: row.get("did"),
                handle: row.get("handle"),
                created_at: parse_timestamp(&row.get::<String, _>("created_at"))?,
                takedown_ref: row.get("takedown_ref"),
                deactivated_at: opt_parse_timestamp(row.get::<Option<String>, _>("deactivated_at"))?,
                delete_after: opt_parse_timestamp(row.get::<Option<String>, _>("delete_after"))?,
                suspended_at: opt_parse_timestamp(row.get::<Option<String>, _>("suspended_at"))?,
                desynchronized_at: opt_parse_timestamp(row.get::<Option<String>, _>("desynchronized_at"))?,
                email: row.get("email"),
                password_hash: row.get("password_hash"),
                email_confirmed_at: opt_parse_timestamp(
                    row.get::<Option<String>, _>("email_confirmed_at"),
                )?,
                invites_disabled: Some(crate::db::read_bool(&row, "invites_disabled")?),
            });
        }

        Ok(accounts)
    }

    /// Get a user's position in the signup queue
    ///
    /// Returns the number of deactivated accounts created before this account,
    /// which represents their position in the queue.
    /// Returns None if the account is not found or is not deactivated.
    pub async fn get_signup_queue_position(&self, did: &str) -> PdsResult<i64> {
        // Get the account's creation time
        let account = self.get_account(did).await?;

        // If account is not deactivated, they're not in queue
        if account.deactivated_at.is_none() {
            return Err(PdsError::NotFound(
                "Account is not in signup queue".to_string(),
            ));
        }

        // Count how many deactivated accounts were created before this one
        // (excluding accounts with takedowns, which are moderation actions not queue)
        let position: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM actor
             WHERE deactivated_at IS NOT NULL
             AND takedown_ref IS NULL
             AND created_at < (SELECT created_at FROM actor WHERE did = $1)
             AND did != $1",
        )
        .bind(did)
        .fetch_one(&self.db)
        .await
        .map_err(PdsError::Database)?;

        // Position is 1-indexed (first in queue = position 1)
        Ok(position + 1)
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
        // Use the real migrations directory so the test schema mirrors
        // production. The previous hand-rolled CREATE TABLE block missed
        // the actor/account split landed in commit 87783e3 and silently
        // broke every account-manager test that touched `JOIN actor`.
        {
            use std::sync::Once;
            static INSTALL: Once = Once::new();
            INSTALL.call_once(sqlx::any::install_default_drivers);
        }
        let db = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations")
            .run(&db)
            .await
            .expect("test schema migrations failed");

        // Create minimal test configuration
        let config = Arc::new(ServerConfig {
            service: ServiceConfig {
                hostname: "localhost".to_string(),
                port: 2583,
                service_did: "did:web:localhost".to_string(),
                version: "0.1.0".to_string(),
                blob_upload_limit: 5242880,
                public_url: None,
                max_blob_fetch_size: 50_000_000,
                blob_fetch_timeout_seconds: 30,
                blob_fetch_max_retries: 3,
                accepting_imports: true,
                max_import_size: None,
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
            database: Default::default(),
            authentication: AuthConfig {
                jwt_secret: "test-secret-key-for-testing-only".to_string(),
                repo_signing_key: "test-key".to_string(),
                plc_rotation_key: "b".repeat(64),
                oauth: crate::config::OAuthConfig {
                    client_id: "test-client".to_string(),
                    redirect_uri: "http://localhost:3000/oauth/callback".to_string(),
                    pds_url: "http://localhost:3000".to_string(),
                },
                jwt_sunset_date: "Sat, 31 Dec 2024 23:59:59 GMT".to_string(),
                oauth_migration_guide_url: "https://docs.example.com/oauth-migration".to_string(),
                oauth_features: Default::default(),
            },
            identity: IdentityConfig {
                did_plc_url: "https://plc.directory".to_string(),
                service_handle_domains: vec!["localhost".to_string()],
                did_cache_stale_ttl: 3600,
                did_cache_max_ttl: 86400,
                recovery_did_key: None,
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
                exempt_admin_assets: true,
                buckets_retention_days: 7,
            },
            logging: LoggingConfig {
                level: "info".to_string(),
            },
            federation: crate::config::FederationConfig {
                enabled: false,
                relay_urls: vec![],
                appview_url: None,
                firehose_enabled: false,
                crawl_enabled: false,
                public_url: None,
                auto_stream_events: false,
                peer_pds: vec![],
            },
            validation_mode: crate::validation::ValidationMode::Optimistic,
            distributed_state_mode: Default::default(),
            maintenance_pool: Default::default(),
            gc_sweep: Default::default(),
            bind_audit_orphan_marker: Default::default(),
            blob_metadata: Default::default(),
            entryway: None,
            lexicon: crate::config::LexiconConfig::default(),
            kryphocron: crate::config::KryphocronConfig::default(),
        });

        AccountManager::new(db, config)
    }

    /// v0.8 arc 3 (#184) — Gate 1 of the email `:`-reject. A `did:`-leading
    /// email can no longer be created, so the login resolver's `did:`-prefix
    /// branch is unambiguously a DID (M4/M5 no-fallback invariant). Asserts
    /// the charset-specific message fires, fires *before* the `@` check
    /// (ordering / message uniformity, M-5), and that ordinary emails pass.
    #[tokio::test]
    async fn validate_email_rejects_colon_local_part() {
        let manager = create_test_manager().await;

        match manager.validate_email("did:foo@example.com").unwrap_err() {
            PdsError::Validation(msg) => {
                assert_eq!(msg, "Email address must not contain ':'")
            }
            other => panic!("expected Validation, got {other:?}"),
        }

        // ':' is rejected before the '@' check: "did:foo" has a ':' and no
        // '@', and must still surface the charset message (not "Invalid
        // email format") — proving the guard ordering.
        match manager.validate_email("did:foo").unwrap_err() {
            PdsError::Validation(msg) => {
                assert_eq!(msg, "Email address must not contain ':'")
            }
            other => panic!("expected Validation, got {other:?}"),
        }

        // No regression: ordinary emails still validate.
        assert!(manager.validate_email("alice@example.com").is_ok());
    }

    /// v0.8 arc 3 (#184) §6.1/§6.2/§6.3 — the DID-identifier resolver branch.
    /// Positive: a DID identifier resolves the local account. Negative +
    /// malformed: a `did:`-prefixed miss is a real miss (no syntax check,
    /// no handle/email fallback), surfacing `get_account`'s NotFound.
    #[tokio::test]
    async fn get_account_by_identifier_routes_did_to_by_did_lookup() {
        let manager = create_test_manager().await;
        let account = manager
            .create_account(
                "s8user".to_string(),
                Some("s8@example.com".to_string()),
                "password123".to_string(),
                None,
                None,
            )
            .await
            .unwrap();

        // §6.1 positive — DID identifier resolves the account.
        let by_did = manager
            .get_account_by_identifier(&account.did)
            .await
            .unwrap();
        assert_eq!(by_did.did, account.did);
        assert_eq!(by_did.handle, account.handle);

        // §6.2 negative + malformed — all `did:`-prefixed misses are real
        // misses (prefix-only detection, no syntax validation).
        for miss in ["did:plc:doesnotexist", "did:", "did:%@!"] {
            match manager.get_account_by_identifier(miss).await {
                Err(PdsError::NotFound(msg)) => {
                    // §6.3 — it is `get_account`'s NotFound ("Account not
                    // found"), i.e. the DID branch did not fall through to
                    // the handle/email lookups.
                    assert_eq!(msg, "Account not found", "miss: {miss}");
                }
                other => panic!("expected NotFound for {miss}, got {other:?}"),
            }
        }
    }

    /// v0.8 arc 3 (#184) §6.4 — per-caller DID smoke tripwires. The DID
    /// identifier must carry through all three callers of
    /// `get_account_by_identifier` to *past* the resolver (an `Authentication`
    /// / `Ok`, never a `NotFound`/404) — proving the funnel, not just the
    /// resolver in isolation.
    #[tokio::test]
    async fn did_identifier_funnels_through_all_three_callers() {
        let manager = create_test_manager().await;
        let account = manager
            .create_account(
                "s8funnel".to_string(),
                Some("s8funnel@example.com".to_string()),
                "password123".to_string(),
                None,
                None,
            )
            .await
            .unwrap();

        // login: DID resolves → password check fails → Authentication
        // (got past the resolver; did NOT 404).
        match manager.login(&account.did, "wrong-password").await {
            Err(PdsError::Authentication(_)) => {}
            other => panic!("login(did, wrong): expected Authentication, got {other:?}"),
        }

        // login_with_app_password: DID resolves → no app password matches →
        // Authentication (not NotFound).
        match manager
            .login_with_app_password(&account.did, "wrong-app-password")
            .await
        {
            Err(PdsError::Authentication(_)) => {}
            other => panic!(
                "login_with_app_password(did, wrong): expected Authentication, got {other:?}"
            ),
        }

        // generate_password_reset_token: DID resolves → account has an email
        // → Ok (past the resolver).
        assert!(
            manager
                .generate_password_reset_token(&account.did)
                .await
                .is_ok(),
            "generate_password_reset_token(did) must resolve a local account with an email",
        );
    }

    #[tokio::test]
    async fn test_cleanup_expired_sessions() {
        let manager = create_test_manager().await;
        let now = Utc::now();

        // Create a test account
        let did = "did:web:test.localhost";
        // Account / actor split: `handle` lives on `actor`, the secrets
        // and email live on `account`. Both rows are required because
        // `account.did` foreign-keys into `actor.did`.
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind(did)
            .bind("testuser")
            .bind(now.to_rfc3339())
            .execute(&manager.db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO account (did, email, password_hash) VALUES ($1, $2, $3)")
            .bind(did)
            .bind("test@example.com")
            .bind("hash")
            .execute(&manager.db)
            .await
            .unwrap();

        // Insert expired session (expired 1 hour ago)
        let expired_time = now - Duration::hours(1);
        sqlx::query(
            "INSERT INTO session (id, did, access_token, refresh_token, created_at, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind("expired-session-1")
        .bind(did)
        .bind("expired-access-token-1")
        .bind("expired-refresh-token-1")
        .bind((now - Duration::hours(2)).to_rfc3339())
        .bind(expired_time.to_rfc3339())
        .execute(&manager.db)
        .await
        .unwrap();

        // Insert valid session (expires in 1 hour)
        let future_time = now + Duration::hours(1);
        sqlx::query(
            "INSERT INTO session (id, did, access_token, refresh_token, created_at, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind("valid-session-1")
        .bind(did)
        .bind("valid-access-token-1")
        .bind("valid-refresh-token-1")
        .bind(now.to_rfc3339())
        .bind(future_time.to_rfc3339())
        .execute(&manager.db)
        .await
        .unwrap();

        // Insert expired refresh token
        sqlx::query(
            "INSERT INTO refresh_token (id, did, token, created_at, expires_at, used)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind("expired-refresh-1")
        .bind(did)
        .bind("old-refresh-token-1")
        .bind((now - Duration::days(200)).to_rfc3339())
        .bind((now - Duration::days(20)).to_rfc3339())
        .bind(false)
        .execute(&manager.db)
        .await
        .unwrap();

        // Insert valid refresh token
        sqlx::query(
            "INSERT INTO refresh_token (id, did, token, created_at, expires_at, used)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind("valid-refresh-1")
        .bind(did)
        .bind("valid-refresh-token-1")
        .bind(now.to_rfc3339())
        .bind((now + Duration::days(180)).to_rfc3339())
        .bind(false)
        .execute(&manager.db)
        .await
        .unwrap();

        // Run cleanup
        let (sessions_deleted, refresh_tokens_deleted) =
            manager.cleanup_expired_sessions().await.unwrap();

        // Verify counts
        assert_eq!(sessions_deleted, 1, "Should delete 1 expired session");
        assert_eq!(
            refresh_tokens_deleted, 1,
            "Should delete 1 expired refresh token"
        );

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
        // Account / actor split: `handle` lives on `actor`, the secrets
        // and email live on `account`. Both rows are required because
        // `account.did` foreign-keys into `actor.did`.
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind(did)
            .bind("testuser")
            .bind(now.to_rfc3339())
            .execute(&manager.db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO account (did, email, password_hash) VALUES ($1, $2, $3)")
            .bind(did)
            .bind("test@example.com")
            .bind("hash")
            .execute(&manager.db)
            .await
            .unwrap();

        // Insert only valid session
        let future_time = now + Duration::hours(1);
        sqlx::query(
            "INSERT INTO session (id, did, access_token, refresh_token, created_at, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind("valid-session")
        .bind(did)
        .bind("valid-token")
        .bind("valid-refresh")
        .bind(now.to_rfc3339())
        .bind(future_time.to_rfc3339())
        .execute(&manager.db)
        .await
        .unwrap();

        // Run cleanup
        let (sessions_deleted, refresh_tokens_deleted) =
            manager.cleanup_expired_sessions().await.unwrap();

        // Verify no deletions
        assert_eq!(sessions_deleted, 0, "Should not delete any sessions");
        assert_eq!(
            refresh_tokens_deleted, 0,
            "Should not delete any refresh tokens"
        );
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
        assert!(!passwords[0].privileged);

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
        let row = sqlx::query("SELECT app_password_name FROM session WHERE id = $1")
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
            "SELECT COUNT(*) FROM session WHERE did = $1 AND app_password_name = $2",
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
            "SELECT COUNT(*) FROM session WHERE did = $1 AND app_password_name = $2",
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
        assert!(validated.is_app_password);

        // Create regular session for comparison
        let (_account, regular_session) = manager.login("testuser", "password123").await.unwrap();

        let validated_regular = manager
            .validate_access_token(&regular_session.access_token)
            .await
            .unwrap();

        assert_eq!(validated_regular.did, account.did);
        assert!(!validated_regular.is_app_password);
    }

    #[tokio::test]
    async fn test_update_handle() {
        let manager = setup_test_db().await;

        // Create test account
        let account = manager
            .create_account("alice".to_string(), None, "password123".to_string(), None, None)
            .await
            .unwrap();

        assert_eq!(account.handle, Some("alice".to_string()));

        // Update handle to new value
        let old_handle = manager
            .update_handle(&account.did, "alice-new")
            .await
            .unwrap();

        assert_eq!(old_handle, "alice");

        // Verify handle was updated in database
        let updated_account = manager.get_account(&account.did).await.unwrap();
        assert_eq!(updated_account.handle, Some("alice-new".to_string()));

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
        let _account1 = manager
            .create_account("alice".to_string(), None, "password123".to_string(), None, None)
            .await
            .unwrap();

        let account2 = manager
            .create_account("bob".to_string(), None, "password456".to_string(), None, None)
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
        assert_eq!(bob_account.handle, Some("bob".to_string()));
    }

    #[tokio::test]
    async fn test_update_handle_same_handle() {
        let manager = setup_test_db().await;

        // Create test account
        let account = manager
            .create_account("alice".to_string(), None, "password123".to_string(), None, None)
            .await
            .unwrap();

        // Update to same handle (should be no-op)
        let old_handle = manager.update_handle(&account.did, "alice").await.unwrap();

        assert_eq!(old_handle, "alice");

        // Verify handle unchanged
        let updated_account = manager.get_account(&account.did).await.unwrap();
        assert_eq!(updated_account.handle, Some("alice".to_string()));
    }

    #[tokio::test]
    async fn test_update_handle_invalid_format() {
        let manager = setup_test_db().await;

        // Create test account
        let account = manager
            .create_account("alice".to_string(), None, "password123".to_string(), None, None)
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
        assert_eq!(unchanged_account.handle, Some("alice".to_string()));
    }

    // LB-1 / chainlink #128: manager `_in_tx` variants must be
    // rollback-safe so handlers can wrap them with chain appends in
    // a single transaction. The pool-API wrappers (`takedown_account`,
    // `update_email`) commit unconditionally; the `_in_tx` variants
    // must let the caller decide.

    #[tokio::test]
    async fn takedown_account_in_tx_rolls_back_on_caller_rollback() {
        let manager = setup_test_db().await;
        let account = manager
            .create_account("alice".to_string(), None, "password123".to_string(), None, None)
            .await
            .unwrap();

        // Confirm starting state: takedown_ref is NULL.
        let pre: Option<String> =
            sqlx::query_scalar("SELECT takedown_ref FROM actor WHERE did = $1")
                .bind(&account.did)
                .fetch_one(&manager.db)
                .await
                .unwrap();
        assert!(pre.is_none(), "actor starts with no takedown_ref");

        // Run the in-tx variant inside a tx that we deliberately
        // roll back. The actor mutation must not land.
        {
            let mut tx = manager.db.begin().await.unwrap();
            AccountManager::takedown_account_in_tx(&mut tx, &account.did, "test_ref")
                .await
                .unwrap();
            tx.rollback().await.unwrap();
        }

        let post: Option<String> =
            sqlx::query_scalar("SELECT takedown_ref FROM actor WHERE did = $1")
                .bind(&account.did)
                .fetch_one(&manager.db)
                .await
                .unwrap();
        assert!(
            post.is_none(),
            "rolled-back tx must not land takedown_ref"
        );
    }

    #[tokio::test]
    async fn takedown_account_in_tx_commits_on_caller_commit() {
        let manager = setup_test_db().await;
        let account = manager
            .create_account("alice".to_string(), None, "password123".to_string(), None, None)
            .await
            .unwrap();

        let mut tx = manager.db.begin().await.unwrap();
        AccountManager::takedown_account_in_tx(&mut tx, &account.did, "committed_ref")
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let post: Option<String> =
            sqlx::query_scalar("SELECT takedown_ref FROM actor WHERE did = $1")
                .bind(&account.did)
                .fetch_one(&manager.db)
                .await
                .unwrap();
        assert_eq!(post.as_deref(), Some("committed_ref"));
    }

    #[tokio::test]
    async fn update_email_in_tx_rolls_back_on_caller_rollback() {
        let manager = setup_test_db().await;
        let _account = manager
            .create_account(
                "alice".to_string(),
                Some("alice@old.example".to_string()),
                "password123".to_string(),
                None,
                        None,
            )
            .await
            .unwrap();
        let did = manager
            .resolve_at_identifier_to_did("alice")
            .await
            .unwrap();

        {
            let mut tx = manager.db.begin().await.unwrap();
            AccountManager::update_email_in_tx(&mut tx, &did, "alice@new.example")
                .await
                .unwrap();
            tx.rollback().await.unwrap();
        }

        let email: Option<String> = sqlx::query_scalar("SELECT email FROM account WHERE did = $1")
            .bind(&did)
            .fetch_one(&manager.db)
            .await
            .unwrap();
        assert_eq!(
            email.as_deref(),
            Some("alice@old.example"),
            "rolled-back tx must not land email update"
        );
    }

    #[tokio::test]
    async fn update_email_in_tx_uniqueness_check_inside_tx() {
        // Two accounts. Trying to set account2's email to account1's
        // email must fail with PdsError::Validation, even when the
        // attempt happens inside a caller-managed tx. The check sees
        // the pre-tx state.
        let manager = setup_test_db().await;
        let _alice = manager
            .create_account(
                "alice".to_string(),
                Some("alice@example".to_string()),
                "password123".to_string(),
                None,
                        None,
            )
            .await
            .unwrap();
        let _bob = manager
            .create_account(
                "bob".to_string(),
                Some("bob@example".to_string()),
                "password456".to_string(),
                None,
                        None,
            )
            .await
            .unwrap();
        let bob_did = manager.resolve_at_identifier_to_did("bob").await.unwrap();

        let mut tx = manager.db.begin().await.unwrap();
        let result =
            AccountManager::update_email_in_tx(&mut tx, &bob_did, "alice@example").await;
        match result {
            Err(PdsError::Validation(_)) => {}
            other => panic!("expected Validation error, got {:?}", other),
        }
        // Drop the tx without commit.
        drop(tx);
    }

    // LB-1 Session 12 / chainlink #129: rollback tests for the new
    // AccountManager `_in_tx` variants. Each test opens a transaction,
    // calls the variant, deliberately rolls back, and asserts the
    // mutation didn't land.

    #[tokio::test]
    async fn update_handle_in_tx_rolls_back_on_caller_rollback() {
        let manager = setup_test_db().await;
        let _account = manager
            .create_account("alice".to_string(), None, "password123".to_string(), None, None)
            .await
            .unwrap();
        let did = manager.resolve_at_identifier_to_did("alice").await.unwrap();

        {
            let mut tx = manager.db.begin().await.unwrap();
            AccountManager::update_handle_in_tx(&mut tx, &did, "alice-renamed")
                .await
                .unwrap();
            tx.rollback().await.unwrap();
        }

        let account = manager.get_account(&did).await.unwrap();
        assert_eq!(account.handle.as_deref(), Some("alice"));
    }

    #[tokio::test]
    async fn update_password_in_tx_rolls_back_on_caller_rollback() {
        let manager = setup_test_db().await;
        let _account = manager
            .create_account("alice".to_string(), None, "original-password".to_string(), None, None)
            .await
            .unwrap();
        let did = manager.resolve_at_identifier_to_did("alice").await.unwrap();

        let original_hash: String = sqlx::query_scalar(
            "SELECT password_hash FROM account WHERE did = $1",
        )
        .bind(&did)
        .fetch_one(&manager.db)
        .await
        .unwrap();

        {
            let mut tx = manager.db.begin().await.unwrap();
            AccountManager::update_password_in_tx(&mut tx, &did, "new-password-x")
                .await
                .unwrap();
            tx.rollback().await.unwrap();
        }

        let post_hash: String = sqlx::query_scalar(
            "SELECT password_hash FROM account WHERE did = $1",
        )
        .bind(&did)
        .fetch_one(&manager.db)
        .await
        .unwrap();
        assert_eq!(post_hash, original_hash, "password hash unchanged after rollback");
    }

    #[tokio::test]
    async fn delete_account_permanent_in_tx_rolls_back_on_caller_rollback() {
        let manager = setup_test_db().await;
        let _account = manager
            .create_account("alice".to_string(), None, "password123".to_string(), None, None)
            .await
            .unwrap();
        let did = manager.resolve_at_identifier_to_did("alice").await.unwrap();

        {
            let mut tx = manager.db.begin().await.unwrap();
            AccountManager::delete_account_permanent_in_tx(&mut tx, &did)
                .await
                .unwrap();
            tx.rollback().await.unwrap();
        }

        // Account still exists post-rollback.
        let account = manager.get_account(&did).await.unwrap();
        assert_eq!(account.handle.as_deref(), Some("alice"));
    }

    #[tokio::test]
    async fn activate_deactivate_reactivate_in_tx_roll_back_on_caller_rollback() {
        let manager = setup_test_db().await;
        let _account = manager
            .create_account("alice".to_string(), None, "password123".to_string(), None, None)
            .await
            .unwrap();
        let did = manager.resolve_at_identifier_to_did("alice").await.unwrap();

        // Pre-seed takedown_ref so activate has something to clear.
        sqlx::query("UPDATE actor SET takedown_ref = 'pre' WHERE did = $1")
            .bind(&did)
            .execute(&manager.db)
            .await
            .unwrap();

        // activate_in_tx rollback
        {
            let mut tx = manager.db.begin().await.unwrap();
            AccountManager::activate_account_in_tx(&mut tx, &did)
                .await
                .unwrap();
            tx.rollback().await.unwrap();
        }
        let takedown: Option<String> =
            sqlx::query_scalar("SELECT takedown_ref FROM actor WHERE did = $1")
                .bind(&did)
                .fetch_one(&manager.db)
                .await
                .unwrap();
        assert_eq!(takedown.as_deref(), Some("pre"));

        // deactivate_in_tx rollback
        {
            let mut tx = manager.db.begin().await.unwrap();
            AccountManager::deactivate_account_in_tx(&mut tx, &did)
                .await
                .unwrap();
            tx.rollback().await.unwrap();
        }
        let deactivated_at: Option<String> =
            sqlx::query_scalar("SELECT deactivated_at FROM actor WHERE did = $1")
                .bind(&did)
                .fetch_one(&manager.db)
                .await
                .unwrap();
        assert!(
            deactivated_at.is_none(),
            "deactivate rolled back; deactivated_at remains NULL"
        );
    }

    #[tokio::test]
    async fn generate_password_reset_token_in_tx_rolls_back_on_caller_rollback() {
        let manager = setup_test_db().await;
        let _account = manager
            .create_account(
                "alice".to_string(),
                Some("alice@example".to_string()),
                "password123".to_string(),
                None,
                        None,
            )
            .await
            .unwrap();
        let did = manager.resolve_at_identifier_to_did("alice").await.unwrap();

        {
            let mut tx = manager.db.begin().await.unwrap();
            let _token =
                AccountManager::generate_password_reset_token_in_tx(&mut tx, &did)
                    .await
                    .unwrap();
            tx.rollback().await.unwrap();
        }

        let token_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM email_token WHERE did = $1 AND purpose = 'reset_password'",
        )
        .bind(&did)
        .fetch_one(&manager.db)
        .await
        .unwrap();
        assert_eq!(token_count, 0, "rolled-back tx must not leave a token row");
    }

    /// Arc 13 §6.4 Step 0.7 — key separation completion.
    /// New accounts MUST populate `plc_keys.atproto_signing_key`
    /// with a non-empty value that is byte-distinct from the
    /// PDS-wide rotation key in config (§6.3.2). The rotation key
    /// column was dropped in Step 0.7.1; what's stored per-account
    /// is *only* the per-actor signing key.
    #[tokio::test]
    async fn test_create_account_populates_distinct_atproto_signing_key() {
        let manager = setup_test_db().await;

        let account = manager
            .create_account(
                "arc12testuser".to_string(),
                Some("arc12@example.com".to_string()),
                "password123".to_string(),
                None,
                        None,
            )
            .await
            .expect("create_account");

        let row: (String,) = sqlx::query_as(
            "SELECT atproto_signing_key FROM plc_keys WHERE did = $1",
        )
        .bind(&account.did)
        .fetch_one(&manager.db)
        .await
        .expect("plc_keys row");

        let (atproto_signing_key,) = row;
        assert!(
            !atproto_signing_key.is_empty(),
            "atproto_signing_key must be populated for new accounts"
        );
        assert_eq!(
            atproto_signing_key.len(),
            64,
            "atproto_signing_key must be 32-byte hex (64 chars)"
        );
        assert!(
            atproto_signing_key
                .chars()
                .all(|c| c.is_ascii_hexdigit()),
            "atproto_signing_key must be valid hex"
        );
        assert_ne!(
            atproto_signing_key,
            manager.config.authentication.plc_rotation_key,
            "per-actor atproto_signing_key must be byte-distinct from \
             the PDS-wide rotation key (§6.3.2 key separation)"
        );
    }

    /// Arc 18 (chainlink #117 / CF3 recon §G) — `create_actor_signer`
    /// resolves the PER-ACCOUNT signing key from
    /// `plc_keys.atproto_signing_key`, NOT the server-wide
    /// `ctx.config.authentication.repo_signing_key`. Two-account
    /// roundtrip proves the property:
    ///
    /// - Sign the same message with each actor's signer.
    /// - Signatures MUST differ — if they're equal, the helper fell
    ///   back to a single global key (Arc 18 regression).
    ///
    /// Together with the existing
    /// `test_create_account_populates_distinct_atproto_signing_key`
    /// (which proves the column write), this locks in the post-Arc-18
    /// per-account signing invariant. Phase B Scenario 2 carries the
    /// integration-level commit-chain-replay validation; this test is
    /// the focused unit-level check.
    #[tokio::test]
    async fn create_actor_signer_resolves_per_account_key_not_global() {
        use proto_blue::crypto::Signer as _;

        let manager = setup_test_db().await;

        let alice = manager
            .create_account(
                "arc18alice".to_string(),
                Some("alice@arc18.example".to_string()),
                "alice-password-12345".to_string(),
                None,
                None,
            )
            .await
            .expect("create alice");
        let bob = manager
            .create_account(
                "arc18bob".to_string(),
                Some("bob@arc18.example".to_string()),
                "bob-password-12345".to_string(),
                None,
                None,
            )
            .await
            .expect("create bob");

        let signer_alice = crate::api::repo::create_actor_signer(&manager, &alice.did)
            .await
            .expect("alice signer");
        let signer_bob = crate::api::repo::create_actor_signer(&manager, &bob.did)
            .await
            .expect("bob signer");

        let msg = b"arc18-roundtrip-check";
        let sig_alice = signer_alice.sign(msg).expect("alice sign");
        let sig_bob = signer_bob.sign(msg).expect("bob sign");

        assert_ne!(
            sig_alice, sig_bob,
            "Arc 18 invariant violated: create_actor_signer returned the \
             same key for two distinct DIDs. Either the helper is reading \
             a global key (regression to pre-Arc-18 behaviour) or the \
             per-account keys collided (vanishingly improbable)."
        );

        // Cross-check: the key resolved for alice's DID matches the column
        // value populated by alice's create_account, NOT bob's. Proves the
        // helper is DID-keyed, not e.g. last-account-keyed.
        let alice_key_bytes = manager
            .get_atproto_signing_key_bytes(&alice.did)
            .await
            .expect("alice key bytes");
        let alice_kp_from_column =
            proto_blue::crypto::K256Keypair::from_private_key(&alice_key_bytes)
                .expect("alice kp from column");
        let sig_from_column = alice_kp_from_column
            .sign(msg)
            .expect("sign with column-derived keypair");
        assert_eq!(
            sig_alice, sig_from_column,
            "create_actor_signer(alice) must produce the same signature as \
             K256Keypair::from_private_key(plc_keys.atproto_signing_key for alice). \
             A mismatch means the helper is not reading the published per-account key."
        );
    }

    // ============================================================
    // Arc 13 §6.3.6 / Step 3 / chainlink #71 — end-to-end
    // validate → consume → re-consume sequence for the
    // plc_operation email-token surface. Pre-#71 these
    // helpers had no integration-test coverage; the only
    // observation was Phase B Scenario 5 surfacing HTTP 500
    // with no error class.
    // ============================================================

    #[tokio::test]
    async fn plc_operation_token_validate_then_consume_returns_consumed() {
        let manager = setup_test_db().await;
        let account = manager
            .create_account(
                "alicetest".to_string(),
                Some("alice@local".to_string()),
                "TestPassword123!".to_string(),
                None,
                None,
            )
            .await
            .expect("create_account");

        let token = manager
            .generate_plc_operation_token(&account.did)
            .await
            .expect("generate token");
        assert!(!token.is_empty());

        // Step 1: validate-only (NO consume).
        manager
            .validate_plc_operation_token(&account.did, &token)
            .await
            .expect("validate must succeed with fresh token");

        // Step 2: validate AGAIN — should still succeed (token
        // not consumed yet — two-phase property).
        manager
            .validate_plc_operation_token(&account.did, &token)
            .await
            .expect("validate-after-validate still succeeds (two-phase preserves token)");

        // Step 3: consume.
        let result = manager
            .consume_plc_operation_token(&account.did, &token)
            .await;
        assert!(
            matches!(result, ConsumeResult::Consumed),
            "first consume must return Consumed; got {:?}",
            result
        );

        // Step 4: validate post-consume — should fail (token
        // marked used).
        let err = manager
            .validate_plc_operation_token(&account.did, &token)
            .await
            .expect_err("validate-after-consume must fail");
        assert!(
            err.to_string().contains("InvalidToken"),
            "post-consume validate error must contain InvalidToken: {}",
            err
        );

        // Step 5: re-consume — must return AlreadyConsumed (per
        // §6.3.6 round-4 F2 ConsumeResult semantics + §71's
        // re-call → HTTP 409 expectation).
        let re_result = manager
            .consume_plc_operation_token(&account.did, &token)
            .await;
        assert!(
            matches!(re_result, ConsumeResult::AlreadyConsumed),
            "re-consume must return AlreadyConsumed; got {:?}",
            re_result
        );
    }

    #[tokio::test]
    async fn plc_operation_token_validate_rejects_unknown_token() {
        let manager = setup_test_db().await;
        let account = manager
            .create_account(
                "bobtest".to_string(),
                Some("bob@local".to_string()),
                "TestPassword123!".to_string(),
                None,
                None,
            )
            .await
            .expect("create_account");

        let err = manager
            .validate_plc_operation_token(&account.did, "garbage-token")
            .await
            .expect_err("unknown token must reject");
        assert!(err.to_string().contains("InvalidToken"));
    }

    #[tokio::test]
    async fn plc_operation_token_consume_unknown_returns_not_found() {
        let manager = setup_test_db().await;
        let account = manager
            .create_account(
                "carolt".to_string(),
                Some("carol@local".to_string()),
                "TestPassword123!".to_string(),
                None,
                None,
            )
            .await
            .expect("create_account");

        let result = manager
            .consume_plc_operation_token(&account.did, "never-issued")
            .await;
        assert!(
            matches!(result, ConsumeResult::NotFound),
            "consume of unknown token must return NotFound; got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn plc_operation_token_validate_rejects_wrong_did() {
        let manager = setup_test_db().await;
        let alice = manager
            .create_account(
                "alicewrong".to_string(),
                Some("a@local".to_string()),
                "TestPassword123!".to_string(),
                None,
                None,
            )
            .await
            .expect("create alice");
        let bob = manager
            .create_account(
                "bobwrong".to_string(),
                Some("b@local".to_string()),
                "TestPassword123!".to_string(),
                None,
                None,
            )
            .await
            .expect("create bob");

        // Token belongs to alice.
        let token = manager
            .generate_plc_operation_token(&alice.did)
            .await
            .expect("generate token");

        // Bob tries to use it.
        let err = manager
            .validate_plc_operation_token(&bob.did, &token)
            .await
            .expect_err("wrong-did validation must reject");
        assert!(
            err.to_string().contains("InvalidToken"),
            "wrong-did err: {}",
            err
        );
    }

    // ──────────────────────────────────────────────────────────────
    // #69 — did:web handle-join double-dot fix (Arc 12 Phase B
    // Scenario 2 reproducer)
    // ──────────────────────────────────────────────────────────────

    #[test]
    fn join_handle_with_domain_strips_leading_dot() {
        // The config default builds `.{hostname}`; the join
        // must produce `usera.localhost`, not `usera..localhost`.
        assert_eq!(
            join_handle_with_domain("usera", ".localhost"),
            "usera.localhost"
        );
    }

    #[test]
    fn join_handle_with_domain_passes_through_no_leading_dot() {
        // Operator-supplied domain entries without a leading dot
        // pass through unchanged — strip_prefix returns the
        // original.
        assert_eq!(
            join_handle_with_domain("usera", "localhost"),
            "usera.localhost"
        );
    }

    #[test]
    fn join_handle_with_domain_handles_multi_segment_domain() {
        // The most common production shape: a multi-segment
        // domain with a leading dot. The strip targets only the
        // single leading dot, not internal dots.
        assert_eq!(
            join_handle_with_domain("usera", ".example.com"),
            "usera.example.com"
        );
    }
}
