//! Refresh Token Rotation with Replay Detection
//!
//! Implements OAuth 2.1 refresh token rotation per RFC 6749 Section 10.4 and
//! draft-ietf-oauth-security-topics-23 Section 4.13.
//!
//! Security Model:
//! 1. Each refresh generates a NEW refresh token (rotation)
//! 2. Old refresh tokens are stored in `used_refresh_token` table
//! 3. If a used token is presented again = REPLAY ATTACK
//! 4. On replay detection, revoke ALL tokens for that account (breach assumption)
//!
//! This prevents token theft - even if an attacker steals a refresh token,
//! they can only use it once before detection triggers account-wide revocation.

use crate::error::{PdsError, PdsResult};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use tracing::{debug, error, warn};
use uuid::Uuid;

/// Token rotation manager
pub struct TokenRotationManager {
    db: SqlitePool,
}

/// Refresh token rotation result
#[derive(Debug, Serialize, Deserialize)]
pub struct RotationResult {
    /// New access token
    pub access_token: String,

    /// New refresh token (rotated)
    pub refresh_token: String,

    /// Token type (always "Bearer")
    pub token_type: String,

    /// Access token expiration (seconds)
    pub expires_in: i64,

    /// OAuth scopes granted
    pub scope: String,
}

impl TokenRotationManager {
    /// Create a new TokenRotationManager
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }

    /// Rotate a refresh token
    ///
    /// This is the critical security operation. Steps:
    /// 1. Verify the refresh token exists and is valid
    /// 2. Check if token was already used (replay attack detection)
    /// 3. Store old token in used_refresh_token table
    /// 4. Generate new access + refresh tokens
    /// 5. Update token record with new tokens
    ///
    /// # Security Notes
    /// - If token was already used (found in used_refresh_token), this is a replay attack
    /// - On replay detection, revoke ALL tokens for that account
    /// - This assumes the account may be compromised
    ///
    /// # Arguments
    /// * `refresh_token` - The refresh token to rotate
    /// * `client_id` - OAuth client ID (for validation)
    ///
    /// # Returns
    /// New access and refresh tokens, or error on replay detection
    pub async fn rotate_token(
        &self,
        refresh_token: &str,
        client_id: &str,
    ) -> PdsResult<RotationResult> {
        // Step 1: Check if this token was already used (replay attack)
        if self.is_token_used(refresh_token).await? {
            // SECURITY ALERT: Replay attack detected!
            // An attacker is trying to reuse an old refresh token

            warn!(
                "🚨 REPLAY ATTACK DETECTED: Refresh token already used: {}",
                &refresh_token[..8]
            );

            // Get the account DID from the used token
            let did = self.get_did_from_used_token(refresh_token).await?;

            // Revoke ALL tokens for this account (breach assumption)
            self.revoke_all_tokens_for_account(&did).await?;

            error!(
                "🚨 Revoked all tokens for account {} due to replay attack",
                did
            );

            return Err(PdsError::Authentication(
                "Refresh token replay detected - all tokens revoked".to_string(),
            ));
        }

        // Step 2: Get the current token record
        let token_record = self.get_token_by_refresh_token(refresh_token).await?;

        // Step 3: Validate client_id matches
        if token_record.client_id != client_id {
            warn!(
                "Client ID mismatch during token rotation: expected {}, got {}",
                token_record.client_id, client_id
            );
            return Err(PdsError::Authentication("Invalid client".to_string()));
        }

        // Step 4: Check if token is expired
        if token_record.expires_at < Utc::now() {
            return Err(PdsError::Authentication("Refresh token expired".to_string()));
        }

        // Step 5: Mark old token as used
        self.store_used_token(
            refresh_token,
            &token_record.token_id,
            &token_record.did,
        )
        .await?;

        // Step 6: Generate new tokens
        let new_access_token = format!("at_{}", Uuid::new_v4().to_string().replace("-", ""));
        let new_refresh_token = format!("rt_{}", Uuid::new_v4().to_string().replace("-", ""));

        // Step 7: Calculate new expiration (access token: 1 hour, refresh: 90 days)
        let now = Utc::now();
        let _access_expires = now + Duration::hours(1); // TODO: store access token expiration
        let refresh_expires = now + Duration::days(90);

        // Step 8: Update token record with new tokens
        sqlx::query(
            r#"
            UPDATE token
            SET current_refresh_token = ?,
                updated_at = ?,
                expires_at = ?
            WHERE token_id = ?
            "#,
        )
        .bind(&new_refresh_token)
        .bind(now)
        .bind(refresh_expires)
        .bind(&token_record.token_id)
        .execute(&self.db)
        .await?;

        debug!(
            "✓ Token rotated successfully for account: {}",
            token_record.did
        );

        // Step 9: Return new tokens
        Ok(RotationResult {
            access_token: new_access_token,
            refresh_token: new_refresh_token,
            token_type: "Bearer".to_string(),
            expires_in: 3600, // 1 hour in seconds
            scope: token_record.scope,
        })
    }

    /// Check if a refresh token was already used
    ///
    /// Queries the `used_refresh_token` table to detect replay attacks.
    async fn is_token_used(&self, refresh_token: &str) -> PdsResult<bool> {
        let result = sqlx::query(
            r#"
            SELECT COUNT(*) as count
            FROM used_refresh_token
            WHERE refresh_token = ?
            "#,
        )
        .bind(refresh_token)
        .fetch_one(&self.db)
        .await?;

        let count: i64 = result.get("count");
        Ok(count > 0)
    }

    /// Store a used refresh token for replay detection
    ///
    /// Adds the token to `used_refresh_token` table with metadata.
    async fn store_used_token(
        &self,
        refresh_token: &str,
        token_id: &str,
        did: &str,
    ) -> PdsResult<()> {
        let now = Utc::now();

        sqlx::query(
            r#"
            INSERT INTO used_refresh_token (
                refresh_token, token_id, did, used_at
            )
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(refresh_token)
        .bind(token_id)
        .bind(did)
        .bind(now)
        .execute(&self.db)
        .await?;

        Ok(())
    }

    /// Get the account DID from a used refresh token
    ///
    /// Used to identify which account to revoke on replay detection.
    async fn get_did_from_used_token(&self, refresh_token: &str) -> PdsResult<String> {
        let row = sqlx::query(
            r#"
            SELECT did
            FROM used_refresh_token
            WHERE refresh_token = ?
            LIMIT 1
            "#,
        )
        .bind(refresh_token)
        .fetch_optional(&self.db)
        .await?
        .ok_or_else(|| {
            PdsError::NotFound("Used token not found (should not happen)".to_string())
        })?;

        Ok(row.get("did"))
    }

    /// Get token record by refresh token
    ///
    /// Retrieves the active token record associated with this refresh token.
    async fn get_token_by_refresh_token(&self, refresh_token: &str) -> PdsResult<TokenRecord> {
        let row = sqlx::query(
            r#"
            SELECT token_id, did, client_id, scope, expires_at, created_at, updated_at
            FROM token
            WHERE current_refresh_token = ?
            "#,
        )
        .bind(refresh_token)
        .fetch_optional(&self.db)
        .await?
        .ok_or_else(|| PdsError::Authentication("Invalid refresh token".to_string()))?;

        Ok(TokenRecord {
            token_id: row.get("token_id"),
            did: row.get("did"),
            client_id: row.get("client_id"),
            scope: row.get("scope"),
            expires_at: row.get("expires_at"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        })
    }

    /// Revoke ALL tokens for an account
    ///
    /// Called on replay attack detection. Assumes account may be compromised.
    /// This is a security-critical operation that invalidates all sessions.
    async fn revoke_all_tokens_for_account(&self, did: &str) -> PdsResult<()> {
        // Delete all tokens for this account
        let result = sqlx::query(
            r#"
            DELETE FROM token
            WHERE did = ?
            "#,
        )
        .bind(did)
        .execute(&self.db)
        .await?;

        let revoked_count = result.rows_affected();

        warn!(
            "🚨 Revoked {} token(s) for account {} due to replay attack",
            revoked_count, did
        );

        Ok(())
    }

    /// Cleanup expired used tokens
    ///
    /// Removes old entries from used_refresh_token table (housekeeping).
    /// Call this periodically (e.g., daily) to prevent table growth.
    ///
    /// # Arguments
    /// * `older_than_days` - Remove tokens used more than N days ago
    ///
    /// # Returns
    /// Number of tokens removed
    pub async fn cleanup_used_tokens(&self, older_than_days: i64) -> PdsResult<u64> {
        let cutoff = Utc::now() - Duration::days(older_than_days);

        let result = sqlx::query(
            r#"
            DELETE FROM used_refresh_token
            WHERE used_at < ?
            "#,
        )
        .bind(cutoff)
        .execute(&self.db)
        .await?;

        let removed = result.rows_affected();

        if removed > 0 {
            debug!(
                "Cleaned up {} used refresh tokens older than {} days",
                removed, older_than_days
            );
        }

        Ok(removed)
    }
}

/// Token record (internal representation)
#[derive(Debug)]
struct TokenRecord {
    token_id: String,
    did: String,
    client_id: String,
    scope: String,
    expires_at: DateTime<Utc>,
    #[allow(dead_code)] // Stored in DB for audit trail
    created_at: DateTime<Utc>,
    #[allow(dead_code)] // Stored in DB for audit trail
    updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rotation_result_serialization() {
        let result = RotationResult {
            access_token: "at_test123".to_string(),
            refresh_token: "rt_test456".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
            scope: "atproto:*".to_string(),
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("access_token"));
        assert!(json.contains("refresh_token"));

        let deserialized: RotationResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.access_token, "at_test123");
        assert_eq!(deserialized.refresh_token, "rt_test456");
        assert_eq!(deserialized.expires_in, 3600);
    }
}
