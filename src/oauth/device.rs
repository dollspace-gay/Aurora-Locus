//! Device Management for OAuth Multi-Device Support

// Allow dead_code - public APIs defined for future feature completion
#![allow(dead_code)]
//!
//! Implements device tracking, registration, and revocation per ATProto OAuth spec.
//! Each device represents a unique client that can maintain its own OAuth session
//! with DPoP key binding for security.

use crate::error::{PdsError, PdsResult};
use crate::oauth::models::{Device, DeviceData, DeviceInfo, DeviceListResponse};
use chrono::Utc;
use sqlx::{Row, SqlitePool};
use tracing::{debug, warn};
use uuid::Uuid;

/// Device Manager
///
/// Handles device lifecycle management including:
/// - Device registration with DPoP key binding
/// - Device tracking and activity updates
/// - Device revocation for security
/// - Multi-device session management
pub struct DeviceManager {
    db: SqlitePool,
}

impl DeviceManager {
    /// Create a new DeviceManager
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }

    /// Register a new device
    ///
    /// Creates a device record with optional DPoP key binding.
    /// Returns the device ID for future operations.
    ///
    /// # Arguments
    /// * `data` - Device information (session_id, user_agent, etc.)
    ///
    /// # Returns
    /// Device ID (UUID)
    pub async fn create_device(&self, data: DeviceData) -> PdsResult<String> {
        let device_id = Uuid::new_v4().to_string();
        let now = Utc::now();

        sqlx::query(
            r#"
            INSERT INTO device (
                id, session_id, user_agent, ip_address, last_seen_at,
                dpop_public_key, created_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&device_id)
        .bind(&data.session_id)
        .bind(&data.user_agent)
        .bind(&data.ip_address)
        .bind(data.last_seen_at)
        .bind(&data.dpop_public_key)
        .bind(now)
        .execute(&self.db)
        .await?;

        debug!(
            "Created device: {} for session: {}",
            device_id, data.session_id
        );

        Ok(device_id)
    }

    /// Get device by ID
    ///
    /// # Arguments
    /// * `device_id` - Device identifier
    ///
    /// # Returns
    /// Device data if found
    pub async fn get_device(&self, device_id: &str) -> PdsResult<Device> {
        let row = sqlx::query(
            r#"
            SELECT id, session_id, user_agent, ip_address, last_seen_at,
                   dpop_public_key, created_at
            FROM device
            WHERE id = ?
            "#,
        )
        .bind(device_id)
        .fetch_optional(&self.db)
        .await?
        .ok_or_else(|| PdsError::NotFound(format!("Device not found: {}", device_id)))?;

        Ok(Device {
            id: row.get("id"),
            session_id: row.get("session_id"),
            user_agent: row.get("user_agent"),
            ip_address: row.get("ip_address"),
            last_seen_at: row.get("last_seen_at"),
            dpop_public_key: row.get("dpop_public_key"),
            created_at: row.get("created_at"),
        })
    }

    /// Update device activity
    ///
    /// Updates device metadata (user agent, IP, last seen timestamp).
    /// Used to track device usage and detect suspicious activity.
    ///
    /// # Arguments
    /// * `device_id` - Device to update
    /// * `data` - Updated device information
    pub async fn update_device(&self, device_id: &str, data: DeviceData) -> PdsResult<()> {
        let result = sqlx::query(
            r#"
            UPDATE device
            SET session_id = ?,
                user_agent = ?,
                ip_address = ?,
                last_seen_at = ?,
                dpop_public_key = ?
            WHERE id = ?
            "#,
        )
        .bind(&data.session_id)
        .bind(&data.user_agent)
        .bind(&data.ip_address)
        .bind(data.last_seen_at)
        .bind(&data.dpop_public_key)
        .bind(device_id)
        .execute(&self.db)
        .await?;

        if result.rows_affected() == 0 {
            return Err(PdsError::NotFound(format!(
                "Device not found: {}",
                device_id
            )));
        }

        debug!("Updated device: {}", device_id);

        Ok(())
    }

    /// Remove/revoke a device
    ///
    /// Permanently removes a device record. This will invalidate all tokens
    /// bound to this device.
    ///
    /// # Arguments
    /// * `device_id` - Device to remove
    pub async fn remove_device(&self, device_id: &str) -> PdsResult<()> {
        let result = sqlx::query(
            r#"
            DELETE FROM device
            WHERE id = ?
            "#,
        )
        .bind(device_id)
        .execute(&self.db)
        .await?;

        if result.rows_affected() == 0 {
            warn!("Attempted to remove non-existent device: {}", device_id);
            return Err(PdsError::NotFound(format!(
                "Device not found: {}",
                device_id
            )));
        }

        debug!("Removed device: {}", device_id);

        Ok(())
    }

    /// Associate device with account
    ///
    /// Links a device to an account for device management.
    /// Enables "list my devices" and "revoke device" functionality.
    ///
    /// # Arguments
    /// * `did` - Account DID
    /// * `device_id` - Device identifier
    /// * `device_name` - Optional user-defined name
    pub async fn associate_device(
        &self,
        did: &str,
        device_id: &str,
        device_name: Option<String>,
    ) -> PdsResult<i64> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            INSERT INTO account_device (
                did, device_id, authorized_at, device_name, is_active
            )
            VALUES (?, ?, ?, ?, 1)
            "#,
        )
        .bind(did)
        .bind(device_id)
        .bind(now)
        .bind(device_name)
        .execute(&self.db)
        .await?;

        debug!("Associated device {} with account {}", device_id, did);

        Ok(result.last_insert_rowid())
    }

    /// List devices for an account
    ///
    /// Returns all devices authorized for the given account.
    /// Used for "manage devices" functionality.
    ///
    /// # Arguments
    /// * `did` - Account DID
    /// * `limit` - Maximum devices to return
    ///
    /// # Returns
    /// List of device information
    pub async fn list_devices(&self, did: &str, limit: i64) -> PdsResult<DeviceListResponse> {
        let rows = sqlx::query(
            r#"
            SELECT
                d.id,
                ad.device_name,
                d.user_agent,
                d.last_seen_at,
                ad.authorized_at,
                ad.is_active
            FROM account_device ad
            INNER JOIN device d ON ad.device_id = d.id
            WHERE ad.did = ? AND ad.is_active = 1
            ORDER BY d.last_seen_at DESC
            LIMIT ?
            "#,
        )
        .bind(did)
        .bind(limit)
        .fetch_all(&self.db)
        .await?;

        let devices = rows
            .into_iter()
            .map(|row| {
                let user_agent: Option<String> = row.get("user_agent");
                let (device_type, browser, os) = parse_user_agent(user_agent.as_deref());

                DeviceInfo {
                    id: row.get("id"),
                    name: row.get("device_name"),
                    device_type,
                    browser,
                    os,
                    last_seen_at: row.get("last_seen_at"),
                    authorized_at: row.get("authorized_at"),
                    is_current: false, // TODO: Detect current device from request context
                }
            })
            .collect();

        Ok(DeviceListResponse {
            devices,
            cursor: None, // TODO: Implement cursor-based pagination
        })
    }

    /// Revoke device access for an account
    ///
    /// Marks a device as inactive and sets revocation timestamp.
    /// The device record remains for audit purposes.
    ///
    /// # Arguments
    /// * `did` - Account DID
    /// * `device_id` - Device to revoke
    pub async fn revoke_device(&self, did: &str, device_id: &str) -> PdsResult<()> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            UPDATE account_device
            SET is_active = 0, revoked_at = ?
            WHERE did = ? AND device_id = ?
            "#,
        )
        .bind(now)
        .bind(did)
        .bind(device_id)
        .execute(&self.db)
        .await?;

        if result.rows_affected() == 0 {
            return Err(PdsError::NotFound(format!(
                "Device association not found: {}",
                device_id
            )));
        }

        debug!("Revoked device {} for account {}", device_id, did);

        Ok(())
    }

    /// Update device last seen timestamp
    ///
    /// Called on each authenticated request to track device activity.
    ///
    /// # Arguments
    /// * `device_id` - Device to update
    pub async fn touch_device(&self, device_id: &str) -> PdsResult<()> {
        let now = Utc::now();

        sqlx::query(
            r#"
            UPDATE device
            SET last_seen_at = ?
            WHERE id = ?
            "#,
        )
        .bind(now)
        .bind(device_id)
        .execute(&self.db)
        .await?;

        Ok(())
    }

    /// Get DPoP public key for a device
    ///
    /// Retrieves the DPoP public key (JWK) bound to this device.
    /// Used to verify DPoP proofs for token binding.
    ///
    /// # Arguments
    /// * `device_id` - Device identifier
    ///
    /// # Returns
    /// DPoP public key (JWK format) if bound
    pub async fn get_dpop_key(&self, device_id: &str) -> PdsResult<Option<String>> {
        let row = sqlx::query(
            r#"
            SELECT dpop_public_key
            FROM device
            WHERE id = ?
            "#,
        )
        .bind(device_id)
        .fetch_optional(&self.db)
        .await?;

        Ok(row.and_then(|r| r.get("dpop_public_key")))
    }
}

/// Parse user agent string to extract device information
///
/// Simple parser for common browsers and platforms.
/// In production, use a proper user-agent parsing library.
///
/// # Returns
/// (device_type, browser, os)
fn parse_user_agent(ua: Option<&str>) -> (String, Option<String>, Option<String>) {
    let ua = match ua {
        Some(s) => s,
        None => return ("unknown".to_string(), None, None),
    };

    let device_type = if ua.contains("Mobile") || ua.contains("Android") || ua.contains("iPhone") {
        "mobile"
    } else if ua.contains("Tablet") || ua.contains("iPad") {
        "tablet"
    } else {
        "desktop"
    }
    .to_string();

    let browser = if ua.contains("Chrome") && !ua.contains("Edg") {
        Some("Chrome".to_string())
    } else if ua.contains("Safari") && !ua.contains("Chrome") {
        Some("Safari".to_string())
    } else if ua.contains("Firefox") {
        Some("Firefox".to_string())
    } else if ua.contains("Edg") {
        Some("Edge".to_string())
    } else {
        None
    };

    // Order matters: iPhone/iPad UAs traditionally include "Mac OS X" for
    // legacy compatibility, so iOS detection must come before macOS or
    // every iPhone gets misclassified as macOS.
    let os = if ua.contains("Windows") {
        Some("Windows".to_string())
    } else if ua.contains("iPhone") || ua.contains("iPad") || ua.contains("iOS") {
        Some("iOS".to_string())
    } else if ua.contains("Android") {
        Some("Android".to_string())
    } else if ua.contains("Mac OS") {
        Some("macOS".to_string())
    } else if ua.contains("Linux") {
        Some("Linux".to_string())
    } else {
        None
    };

    (device_type, browser, os)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_user_agent_chrome_desktop() {
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
        let (device_type, browser, os) = parse_user_agent(Some(ua));

        assert_eq!(device_type, "desktop");
        assert_eq!(browser, Some("Chrome".to_string()));
        assert_eq!(os, Some("Windows".to_string()));
    }

    #[test]
    fn test_parse_user_agent_safari_mobile() {
        let ua = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";
        let (device_type, browser, os) = parse_user_agent(Some(ua));

        assert_eq!(device_type, "mobile");
        assert_eq!(browser, Some("Safari".to_string()));
        assert_eq!(os, Some("iOS".to_string()));
    }

    #[test]
    fn test_parse_user_agent_firefox_linux() {
        let ua = "Mozilla/5.0 (X11; Linux x86_64; rv:120.0) Gecko/20100101 Firefox/120.0";
        let (device_type, browser, os) = parse_user_agent(Some(ua));

        assert_eq!(device_type, "desktop");
        assert_eq!(browser, Some("Firefox".to_string()));
        assert_eq!(os, Some("Linux".to_string()));
    }

    #[test]
    fn test_parse_user_agent_unknown() {
        let (device_type, browser, os) = parse_user_agent(None);

        assert_eq!(device_type, "unknown");
        assert_eq!(browser, None);
        assert_eq!(os, None);
    }
}
