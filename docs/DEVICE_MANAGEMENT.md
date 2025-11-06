# Device Management Guide

Complete guide for implementing multi-device support with OAuth 2.1 device tokens.

## Table of Contents

- [Overview](#overview)
- [Device Registration](#device-registration)
- [Device Tokens](#device-tokens)
- [Device Management](#device-management)
- [Device Revocation](#device-revocation)
- [Best Practices](#best-practices)
- [API Reference](#api-reference)

## Overview

Aurora Locus supports multi-device authentication where each device has:

- **Unique device token** - Separate OAuth tokens per device
- **Device metadata** - Name, type, last used timestamp
- **Independent DPoP key pair** - Cryptographic key bound to device
- **Granular revocation** - Revoke specific devices without affecting others

### Benefits

- **Security**: Compromised device doesn't affect other devices
- **User Control**: Users can see and manage all signed-in devices
- **Seamless UX**: "Remember this device" for one-tap re-authentication
- **Audit Trail**: Track which device performed which actions

## Device Registration

### Step 1: Generate Device ID

```rust
use uuid::Uuid;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    pub device_type: String,
    pub os: String,
    pub app_version: String,
}

impl DeviceInfo {
    pub fn generate() -> Self {
        Self {
            device_id: Uuid::new_v4().to_string(),
            device_name: get_device_name(),
            device_type: get_device_type(),
            os: get_os_info(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

fn get_device_name() -> String {
    // Platform-specific device name
    #[cfg(target_os = "macos")]
    return std::env::var("HOSTNAME")
        .unwrap_or_else(|_| "Mac".to_string());

    #[cfg(target_os = "windows")]
    return std::env::var("COMPUTERNAME")
        .unwrap_or_else(|_| "Windows PC".to_string());

    #[cfg(target_os = "linux")]
    return std::env::var("HOSTNAME")
        .unwrap_or_else(|_| "Linux".to_string());

    #[cfg(target_os = "ios")]
    return "iPhone".to_string();

    #[cfg(target_os = "android")]
    return "Android".to_string();

    "Unknown Device".to_string()
}

fn get_device_type() -> String {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    return "mobile".to_string();

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    return "desktop".to_string();

    "unknown".to_string()
}

fn get_os_info() -> String {
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}
```

### Step 2: Register Device with PDS

```rust
async fn register_device(
    access_token: &str,
    device_info: &DeviceInfo,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();

    let response = client
        .post("https://pds.example.com/xrpc/com.atproto.device.register")
        .header("Authorization", format!("Bearer {}", access_token))
        .json(device_info)
        .send()
        .await?;

    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("Device registration failed: {}", response.status()).into())
    }
}
```

### Step 3: Store Device Credentials

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceCredentials {
    pub device_id: String,
    pub device_name: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
    pub dpop_private_key: String,
    pub dpop_public_key: String,
}

impl DeviceCredentials {
    pub fn save_to_keychain(&self) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(target_os = "macos")]
        {
            // Use macOS Keychain
            keyring::Entry::new("aurora-locus", &self.device_id)?
                .set_password(&serde_json::to_string(self)?)?;
        }

        #[cfg(target_os = "windows")]
        {
            // Use Windows Credential Manager
            keyring::Entry::new("aurora-locus", &self.device_id)?
                .set_password(&serde_json::to_string(self)?)?;
        }

        #[cfg(target_os = "linux")]
        {
            // Use Secret Service API / gnome-keyring
            keyring::Entry::new("aurora-locus", &self.device_id)?
                .set_password(&serde_json::to_string(self)?)?;
        }

        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            // Use platform-specific secure storage
            save_to_secure_storage(&self.device_id, &serde_json::to_string(self)?)?;
        }

        Ok(())
    }

    pub fn load_from_keychain(device_id: &str) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            let entry = keyring::Entry::new("aurora-locus", device_id)?;
            match entry.get_password() {
                Ok(json) => Ok(Some(serde_json::from_str(&json)?)),
                Err(_) => Ok(None),
            }
        }

        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            load_from_secure_storage(device_id)
        }
    }

    pub fn delete_from_keychain(&self) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            keyring::Entry::new("aurora-locus", &self.device_id)?
                .delete_password()?;
        }

        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            delete_from_secure_storage(&self.device_id)?;
        }

        Ok(())
    }
}
```

## Device Tokens

### Complete Device Authentication Flow

```rust
pub struct DeviceAuthManager {
    device_info: DeviceInfo,
    credentials: Option<DeviceCredentials>,
    oauth_client: OAuthClient,
}

impl DeviceAuthManager {
    pub fn new(oauth_client: OAuthClient) -> Self {
        let device_info = DeviceInfo::generate();

        // Try to load existing credentials
        let credentials = DeviceCredentials::load_from_keychain(&device_info.device_id)
            .ok()
            .flatten();

        Self {
            device_info,
            credentials,
            oauth_client,
        }
    }

    /// Check if device is already authenticated
    pub fn is_authenticated(&self) -> bool {
        self.credentials.is_some()
    }

    /// Perform device authentication (first-time or re-auth)
    pub async fn authenticate(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Step 1: Get authorization URL
        let auth_url = self.oauth_client
            .get_authorization_url("atproto:repo.create atproto:read")?;

        // Step 2: Open browser for user authorization
        open::that(&auth_url)?;
        println!("Opening browser for authorization...");

        // Step 3: Wait for callback with authorization code
        let auth_code = wait_for_oauth_callback().await?;

        // Step 4: Exchange code for tokens
        let tokens = self.oauth_client
            .exchange_code(&auth_code)
            .await?;

        // Step 5: Register device with PDS
        register_device(&tokens.access_token, &self.device_info).await?;

        // Step 6: Store credentials securely
        let credentials = DeviceCredentials {
            device_id: self.device_info.device_id.clone(),
            device_name: self.device_info.device_name.clone(),
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_at: tokens.expires_at,
            dpop_private_key: self.oauth_client.dpop_key.private_key.clone(),
            dpop_public_key: self.oauth_client.dpop_key.public_key.clone(),
        };

        credentials.save_to_keychain()?;
        self.credentials = Some(credentials);

        Ok(())
    }

    /// Make authenticated API request
    pub async fn make_request(
        &mut self,
        endpoint: &str,
        method: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let credentials = self.credentials
            .as_mut()
            .ok_or("Not authenticated")?;

        // Refresh token if needed
        if credentials.expires_at < SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() + 300 {
            self.refresh_token().await?;
        }

        // Make request with DPoP
        self.oauth_client.make_request(
            &credentials.access_token,
            endpoint,
            method,
        ).await
    }

    /// Refresh access token
    async fn refresh_token(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let credentials = self.credentials
            .as_mut()
            .ok_or("Not authenticated")?;

        let new_tokens = self.oauth_client
            .refresh_token(&credentials.refresh_token)
            .await?;

        // Update credentials
        credentials.access_token = new_tokens.access_token;
        credentials.refresh_token = new_tokens.refresh_token;
        credentials.expires_at = new_tokens.expires_at;

        // Save updated credentials
        credentials.save_to_keychain()?;

        Ok(())
    }

    /// Sign out and remove device credentials
    pub fn sign_out(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(credentials) = &self.credentials {
            credentials.delete_from_keychain()?;
        }

        self.credentials = None;
        Ok(())
    }
}
```

## Device Management

### List User's Devices

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct Device {
    pub device_id: String,
    pub device_name: String,
    pub device_type: String,
    pub os: String,
    pub first_seen: String,
    pub last_used: String,
    pub is_current: bool,
}

#[derive(Debug, Deserialize)]
pub struct DeviceListResponse {
    pub devices: Vec<Device>,
    pub cursor: Option<String>,
}

async fn list_devices(
    access_token: &str,
) -> Result<Vec<Device>, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();

    let response: DeviceListResponse = client
        .get("https://pds.example.com/xrpc/com.atproto.device.list")
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await?
        .json()
        .await?;

    Ok(response.devices)
}
```

### Display Device Management UI

```rust
pub async fn show_device_management_ui(
    access_token: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let devices = list_devices(access_token).await?;

    println!("\n=== Your Devices ===\n");

    for device in &devices {
        println!("Device: {}", device.device_name);
        println!("  Type: {}", device.device_type);
        println!("  OS: {}", device.os);
        println!("  Last used: {}", device.last_used);

        if device.is_current {
            println!("  ⭐ Current device");
        }

        println!("  ID: {}", device.device_id);
        println!();
    }

    println!("Commands:");
    println!("  1. Rename device");
    println!("  2. Revoke device");
    println!("  3. Refresh list");
    println!("  4. Back");

    Ok(())
}
```

### Rename Device

```rust
#[derive(Serialize)]
struct RenameDeviceRequest {
    device_id: String,
    new_name: String,
}

async fn rename_device(
    access_token: &str,
    device_id: &str,
    new_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();

    client
        .post("https://pds.example.com/xrpc/com.atproto.device.update")
        .header("Authorization", format!("Bearer {}", access_token))
        .json(&RenameDeviceRequest {
            device_id: device_id.to_string(),
            new_name: new_name.to_string(),
        })
        .send()
        .await?;

    Ok(())
}
```

## Device Revocation

### Revoke Specific Device

```rust
#[derive(Serialize)]
struct RevokeDeviceRequest {
    device_id: String,
}

async fn revoke_device(
    access_token: &str,
    device_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();

    let response = client
        .post("https://pds.example.com/xrpc/com.atproto.device.revoke")
        .header("Authorization", format!("Bearer {}", access_token))
        .json(&RevokeDeviceRequest {
            device_id: device_id.to_string(),
        })
        .send()
        .await?;

    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("Revocation failed: {}", response.status()).into())
    }
}
```

### Revoke All Other Devices

```rust
async fn revoke_all_other_devices(
    access_token: &str,
    current_device_id: &str,
) -> Result<usize, Box<dyn std::error::Error>> {
    let devices = list_devices(access_token).await?;

    let mut revoked_count = 0;

    for device in devices {
        if device.device_id != current_device_id {
            if let Err(e) = revoke_device(access_token, &device.device_id).await {
                eprintln!("Failed to revoke device {}: {}", device.device_name, e);
            } else {
                revoked_count += 1;
            }
        }
    }

    Ok(revoked_count)
}
```

### Handle Revoked Device

```rust
impl DeviceAuthManager {
    pub async fn make_request(
        &mut self,
        endpoint: &str,
        method: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        match self.make_request_internal(endpoint, method).await {
            Ok(response) => Ok(response),
            Err(e) if is_revoked_error(&e) => {
                // Device was revoked - clear credentials and prompt re-auth
                self.sign_out()?;
                Err("Device has been revoked. Please sign in again.".into())
            }
            Err(e) => Err(e),
        }
    }
}

fn is_revoked_error(error: &dyn std::error::Error) -> bool {
    error.to_string().contains("401") ||
    error.to_string().contains("Token revoked") ||
    error.to_string().contains("Device revoked")
}
```

## Best Practices

### Security

1. **Store DPoP keys securely**
   - Use OS keychain/credential manager
   - Never store private keys in plain text
   - Generate new key pair per device

2. **Implement device limits**
   - Warn users when approaching limit (e.g., 10 devices)
   - Auto-revoke oldest unused devices when limit exceeded

3. **Monitor for suspicious activity**
   - Alert on new device sign-in from unusual location
   - Require additional verification for sensitive operations

4. **Regular token rotation**
   - Refresh tokens have limited lifetime (30-90 days)
   - Implement automatic refresh before expiration

### User Experience

1. **Device naming**
   - Auto-generate meaningful names ("John's iPhone", "Work Mac")
   - Allow custom names for easier identification
   - Show device type icons (phone, laptop, tablet)

2. **Last used tracking**
   - Display relative timestamps ("2 hours ago", "Last week")
   - Sort by last used (most recent first)
   - Highlight current device

3. **Revocation warnings**
   - Confirm before revoking devices
   - Explain consequences ("You'll need to sign in again")
   - Provide "Revoke all other devices" option

4. **Re-authentication flow**
   - Detect revoked/expired tokens automatically
   - Provide seamless re-auth without data loss
   - Remember user preferences during re-auth

### Error Handling

```rust
pub enum DeviceError {
    NotAuthenticated,
    DeviceRevoked,
    TokenExpired,
    NetworkError(String),
    ServerError(String),
}

impl DeviceAuthManager {
    pub async fn make_request_with_retry(
        &mut self,
        endpoint: &str,
        method: &str,
        max_retries: u32,
    ) -> Result<String, DeviceError> {
        for attempt in 0..max_retries {
            match self.make_request(endpoint, method).await {
                Ok(response) => return Ok(response),
                Err(e) if is_retryable(&e) && attempt < max_retries - 1 => {
                    tokio::time::sleep(Duration::from_secs(2u64.pow(attempt))).await;
                    continue;
                }
                Err(e) if is_revoked_error(&e) => return Err(DeviceError::DeviceRevoked),
                Err(e) => return Err(DeviceError::NetworkError(e.to_string())),
            }
        }

        Err(DeviceError::NetworkError("Max retries exceeded".to_string()))
    }
}

fn is_retryable(error: &dyn std::error::Error) -> bool {
    let msg = error.to_string();
    msg.contains("timeout") ||
    msg.contains("connection") ||
    msg.contains("500") ||
    msg.contains("502") ||
    msg.contains("503")
}
```

## API Reference

### Device Registration

```http
POST /xrpc/com.atproto.device.register
Authorization: Bearer <access_token>
Content-Type: application/json

{
  "device_id": "uuid-v4",
  "device_name": "John's iPhone",
  "device_type": "mobile",
  "os": "iOS 17.0",
  "app_version": "1.0.0"
}
```

### List Devices

```http
GET /xrpc/com.atproto.device.list
Authorization: Bearer <access_token>

Response:
{
  "devices": [
    {
      "device_id": "uuid-v4",
      "device_name": "John's iPhone",
      "device_type": "mobile",
      "os": "iOS 17.0",
      "first_seen": "2025-01-15T10:00:00Z",
      "last_used": "2025-01-20T14:30:00Z",
      "is_current": true
    }
  ],
  "cursor": null
}
```

### Update Device

```http
POST /xrpc/com.atproto.device.update
Authorization: Bearer <access_token>
Content-Type: application/json

{
  "device_id": "uuid-v4",
  "new_name": "Personal iPhone"
}
```

### Revoke Device

```http
POST /xrpc/com.atproto.device.revoke
Authorization: Bearer <access_token>
Content-Type: application/json

{
  "device_id": "uuid-v4"
}
```

### Get Device Info

```http
GET /xrpc/com.atproto.device.get?device_id=uuid-v4
Authorization: Bearer <access_token>

Response:
{
  "device_id": "uuid-v4",
  "device_name": "John's iPhone",
  "device_type": "mobile",
  "os": "iOS 17.0",
  "app_version": "1.0.0",
  "first_seen": "2025-01-15T10:00:00Z",
  "last_used": "2025-01-20T14:30:00Z",
  "token_count": 1,
  "is_current": true
}
```

## Additional Resources

- [OAuth Client Guide](OAUTH_CLIENT_GUIDE.md)
- [Migration Guide](MIGRATION_GUIDE.md)
- [Security Best Practices](SECURITY.md)
- [API Reference](API_REFERENCE.md)
