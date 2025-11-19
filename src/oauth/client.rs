// Allow dead_code - OAuth client management for future use
#![allow(dead_code)]

//! OAuth Client Management
//!
//! Implements OAuth 2.1 client registration, validation, and authorization tracking.
//! For Phase 1, clients are statically configured via TOML or environment variables.
//! Future phases may support dynamic client registration per RFC 7591.

use crate::error::{PdsError, PdsResult};
use crate::oauth::models::{AuthorizedClientInfo, ClientListResponse, OAuthClient};
use chrono::Utc;
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;
use tracing::{debug, warn};

/// Client Manager
///
/// Handles OAuth client lifecycle management including:
/// - Static client registration (config-based)
/// - Client ID and redirect URI validation
/// - Authorization tracking (authorized_client table)
/// - Client revocation for security
pub struct ClientManager {
    db: SqlitePool,
    /// Registered OAuth clients (client_id -> client config)
    clients: HashMap<String, OAuthClient>,
}

impl ClientManager {
    /// Create a new ClientManager with registered clients
    ///
    /// # Arguments
    /// * `db` - Database connection pool
    /// * `clients` - List of registered OAuth clients
    pub fn new(db: SqlitePool, clients: Vec<OAuthClient>) -> Self {
        let client_map = clients
            .into_iter()
            .map(|c| (c.client_id.clone(), c))
            .collect();

        Self {
            db,
            clients: client_map,
        }
    }

    /// Get client configuration by client ID
    ///
    /// # Arguments
    /// * `client_id` - OAuth client identifier
    ///
    /// # Returns
    /// Client configuration if registered
    pub fn get_client(&self, client_id: &str) -> Option<&OAuthClient> {
        self.clients.get(client_id)
    }

    /// Validate client ID
    ///
    /// Checks if the client_id is registered in the static configuration.
    ///
    /// # Arguments
    /// * `client_id` - Client identifier to validate
    ///
    /// # Returns
    /// Ok(()) if valid, Err if not registered
    pub fn validate_client_id(&self, client_id: &str) -> PdsResult<()> {
        if self.clients.contains_key(client_id) {
            Ok(())
        } else {
            warn!("Invalid client_id: {}", client_id);
            Err(PdsError::Authentication(format!(
                "Unknown client: {}",
                client_id
            )))
        }
    }

    /// Validate redirect URI against client whitelist
    ///
    /// OAuth 2.1 requires strict redirect_uri validation to prevent phishing.
    /// The redirect_uri must be an exact match against the registered whitelist.
    ///
    /// # Arguments
    /// * `client_id` - Client identifier
    /// * `redirect_uri` - Redirect URI to validate
    ///
    /// # Returns
    /// Ok(()) if redirect_uri is whitelisted, Err otherwise
    pub fn validate_redirect_uri(&self, client_id: &str, redirect_uri: &str) -> PdsResult<()> {
        let client = self.get_client(client_id).ok_or_else(|| {
            PdsError::Authentication(format!("Unknown client: {}", client_id))
        })?;

        if client.redirect_uris.contains(&redirect_uri.to_string()) {
            Ok(())
        } else {
            warn!(
                "Invalid redirect_uri for client {}: {}",
                client_id, redirect_uri
            );
            Err(PdsError::Authentication(
                "Invalid redirect_uri for this client".to_string(),
            ))
        }
    }

    /// Store authorized client on first grant
    ///
    /// Creates or updates an authorized_client record when a user grants
    /// permission to an OAuth client. This enables "remember this app" functionality.
    ///
    /// # Arguments
    /// * `did` - Account DID
    /// * `client_id` - OAuth client ID
    /// * `scope` - Granted scopes (space-separated)
    ///
    /// # Returns
    /// Authorized client ID
    pub async fn authorize_client(
        &self,
        did: &str,
        client_id: &str,
        scope: &str,
    ) -> PdsResult<i64> {
        let now = Utc::now();

        // Check if authorization already exists
        let existing = sqlx::query(
            r#"
            SELECT id, is_active
            FROM authorized_client
            WHERE did = ? AND client_id = ?
            "#,
        )
        .bind(did)
        .bind(client_id)
        .fetch_optional(&self.db)
        .await?;

        if let Some(row) = existing {
            let id: i64 = row.get("id");
            let is_active: bool = row.get("is_active");

            if is_active {
                // Update existing authorization
                sqlx::query(
                    r#"
                    UPDATE authorized_client
                    SET scope = ?, last_used_at = ?
                    WHERE id = ?
                    "#,
                )
                .bind(scope)
                .bind(now)
                .bind(id)
                .execute(&self.db)
                .await?;

                debug!(
                    "Updated authorized_client {} for account {} and client {}",
                    id, did, client_id
                );

                return Ok(id);
            } else {
                // Reactivate revoked authorization
                sqlx::query(
                    r#"
                    UPDATE authorized_client
                    SET scope = ?, is_active = 1, revoked_at = NULL, last_used_at = ?
                    WHERE id = ?
                    "#,
                )
                .bind(scope)
                .bind(now)
                .bind(id)
                .execute(&self.db)
                .await?;

                debug!(
                    "Reactivated authorized_client {} for account {} and client {}",
                    id, did, client_id
                );

                return Ok(id);
            }
        }

        // Create new authorization
        let result = sqlx::query(
            r#"
            INSERT INTO authorized_client (
                did, client_id, scope, first_authorized_at, last_used_at, is_active
            )
            VALUES (?, ?, ?, ?, ?, 1)
            "#,
        )
        .bind(did)
        .bind(client_id)
        .bind(scope)
        .bind(now)
        .bind(now)
        .execute(&self.db)
        .await?;

        debug!(
            "Created authorized_client for account {} and client {}",
            did, client_id
        );

        Ok(result.last_insert_rowid())
    }

    /// Update last used timestamp for authorized client
    ///
    /// Called during token refresh to track client activity.
    ///
    /// # Arguments
    /// * `did` - Account DID
    /// * `client_id` - OAuth client ID
    pub async fn touch_authorized_client(&self, did: &str, client_id: &str) -> PdsResult<()> {
        let now = Utc::now();

        sqlx::query(
            r#"
            UPDATE authorized_client
            SET last_used_at = ?
            WHERE did = ? AND client_id = ? AND is_active = 1
            "#,
        )
        .bind(now)
        .bind(did)
        .bind(client_id)
        .execute(&self.db)
        .await?;

        Ok(())
    }

    /// List authorized clients for an account
    ///
    /// Returns all OAuth clients authorized by the user.
    /// Used for "manage app permissions" functionality.
    ///
    /// # Arguments
    /// * `did` - Account DID
    /// * `limit` - Maximum clients to return
    ///
    /// # Returns
    /// List of authorized client information
    pub async fn list_authorized_clients(
        &self,
        did: &str,
        limit: i64,
    ) -> PdsResult<ClientListResponse> {
        let rows = sqlx::query(
            r#"
            SELECT
                client_id,
                scope,
                first_authorized_at,
                last_used_at
            FROM authorized_client
            WHERE did = ? AND is_active = 1
            ORDER BY last_used_at DESC, first_authorized_at DESC
            LIMIT ?
            "#,
        )
        .bind(did)
        .bind(limit)
        .fetch_all(&self.db)
        .await?;

        let clients = rows
            .into_iter()
            .filter_map(|row| {
                let client_id: String = row.get("client_id");
                let scope: String = row.get("scope");

                // Get client configuration
                let client_config = self.get_client(&client_id)?;

                Some(AuthorizedClientInfo {
                    client_id: client_id.clone(),
                    client_name: client_config.client_name.clone(),
                    logo_uri: client_config.logo_uri.clone(),
                    scopes: scope.split_whitespace().map(|s| s.to_string()).collect(),
                    first_authorized_at: row.get("first_authorized_at"),
                    last_used_at: row.get("last_used_at"),
                })
            })
            .collect();

        Ok(ClientListResponse {
            clients,
            cursor: None, // TODO: Implement cursor-based pagination
        })
    }

    /// Revoke client authorization for an account
    ///
    /// Marks a client as inactive and sets revocation timestamp.
    /// This invalidates all tokens issued to this client for this account.
    ///
    /// # Arguments
    /// * `did` - Account DID
    /// * `client_id` - Client to revoke
    pub async fn revoke_client(&self, did: &str, client_id: &str) -> PdsResult<()> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            UPDATE authorized_client
            SET is_active = 0, revoked_at = ?
            WHERE did = ? AND client_id = ?
            "#,
        )
        .bind(now)
        .bind(did)
        .bind(client_id)
        .execute(&self.db)
        .await?;

        if result.rows_affected() == 0 {
            return Err(PdsError::NotFound(format!(
                "Client authorization not found: {}",
                client_id
            )));
        }

        // TODO: Also revoke all active tokens for this client+account combo
        // This would require cascading to the token table

        debug!("Revoked client {} for account {}", client_id, did);

        Ok(())
    }

    /// Check if a client is authorized for an account
    ///
    /// # Arguments
    /// * `did` - Account DID
    /// * `client_id` - Client identifier
    ///
    /// # Returns
    /// True if client is currently authorized
    pub async fn is_client_authorized(&self, did: &str, client_id: &str) -> PdsResult<bool> {
        let result = sqlx::query(
            r#"
            SELECT COUNT(*) as count
            FROM authorized_client
            WHERE did = ? AND client_id = ? AND is_active = 1
            "#,
        )
        .bind(did)
        .bind(client_id)
        .fetch_one(&self.db)
        .await?;

        let count: i64 = result.get("count");
        Ok(count > 0)
    }

    /// Get default scopes for a client
    ///
    /// Returns the default OAuth scopes that should be granted to this client.
    ///
    /// # Arguments
    /// * `client_id` - Client identifier
    ///
    /// # Returns
    /// Default scopes (space-separated string)
    pub fn get_default_scopes(&self, client_id: &str) -> PdsResult<String> {
        let client = self.get_client(client_id).ok_or_else(|| {
            PdsError::Authentication(format!("Unknown client: {}", client_id))
        })?;

        Ok(client.default_scopes.join(" "))
    }

    /// Check if a client is trusted (first-party)
    ///
    /// Trusted clients may skip the consent screen or have additional privileges.
    ///
    /// # Arguments
    /// * `client_id` - Client identifier
    ///
    /// # Returns
    /// True if client is marked as trusted
    pub fn is_trusted_client(&self, client_id: &str) -> bool {
        self.get_client(client_id)
            .map(|c| c.is_trusted)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_client() -> OAuthClient {
        OAuthClient {
            client_id: "https://example.com/client".to_string(),
            client_name: "Test Client".to_string(),
            redirect_uris: vec![
                "https://example.com/callback".to_string(),
                "https://example.com/oauth/callback".to_string(),
            ],
            default_scopes: vec!["atproto:read".to_string(), "atproto:write".to_string()],
            logo_uri: Some("https://example.com/logo.png".to_string()),
            policy_uri: Some("https://example.com/privacy".to_string()),
            is_trusted: false,
        }
    }

    #[test]
    fn test_validate_client_id() {
        let pool = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let client = create_test_client();
        let manager = ClientManager::new(pool, vec![client.clone()]);

        assert!(manager.validate_client_id(&client.client_id).is_ok());
        assert!(manager.validate_client_id("unknown").is_err());
    }

    #[test]
    fn test_validate_redirect_uri() {
        let pool = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let client = create_test_client();
        let manager = ClientManager::new(pool, vec![client.clone()]);

        // Valid redirect URIs
        assert!(manager
            .validate_redirect_uri(&client.client_id, "https://example.com/callback")
            .is_ok());
        assert!(manager
            .validate_redirect_uri(&client.client_id, "https://example.com/oauth/callback")
            .is_ok());

        // Invalid redirect URI
        assert!(manager
            .validate_redirect_uri(&client.client_id, "https://evil.com/callback")
            .is_err());
    }

    #[test]
    fn test_get_default_scopes() {
        let pool = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let client = create_test_client();
        let manager = ClientManager::new(pool, vec![client.clone()]);

        let scopes = manager.get_default_scopes(&client.client_id).unwrap();
        assert_eq!(scopes, "atproto:read atproto:write");
    }

    #[test]
    fn test_is_trusted_client() {
        let pool = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let client = create_test_client();
        let manager = ClientManager::new(pool, vec![client.clone()]);

        assert_eq!(manager.is_trusted_client(&client.client_id), false);
    }
}
