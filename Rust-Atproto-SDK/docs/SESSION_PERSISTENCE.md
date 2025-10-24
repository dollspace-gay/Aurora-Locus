# Session Persistence Guide

This guide explains how to use the session persistence features in the Rust ATProto SDK.

## Overview

The SDK provides automatic session persistence with:
- **File-based storage** - Sessions saved to disk
- **Automatic token refresh** - Tokens refreshed before expiration
- **Multi-account support** - Manage multiple accounts simultaneously
- **Session callbacks** - React to session events
- **Thread-safe** - Safe to use across threads

## Quick Start

```rust
use atproto::session_manager::PersistentSessionManager;
use std::path::PathBuf;

// Create a persistent session manager
let session_manager = PersistentSessionManager::new(
    PathBuf::from("./sessions"),  // Directory to store sessions
    "https://bsky.social".to_string()  // PDS service URL
);
```

## Features

### 1. Automatic Session Persistence

Sessions are automatically saved to disk and can be restored on restart:

```rust
use atproto::types::AtpSessionData;

// After login, store the session
let session_data = AtpSessionData {
    refresh_jwt: "...".to_string(),
    access_jwt: "...".to_string(),
    handle: "alice.bsky.social".to_string(),
    did: "did:plc:xyz123".to_string(),
    email: Some("alice@example.com".to_string()),
    email_confirmed: Some(true),
    email_auth_factor: Some(false),
    active: true,
    status: None,
};

session_manager.store_session(session_data).await?;
```

**Session files** are stored as JSON in the sessions directory:
- Format: `{session_dir}/{did_sanitized}.json`
- Example: `./sessions/did_plc_xyz123.json`

### 2. Automatic Token Refresh

Tokens are automatically refreshed before they expire (within 2 minutes of expiration):

```rust
// Enable auto-refresh (enabled by default)
session_manager.enable_auto_refresh();

// Disable auto-refresh if needed
session_manager.disable_auto_refresh();

// Manual refresh
session_manager.refresh_token().await?;
```

**How it works:**
1. JWT expiration is extracted from the access token
2. Before each request, expiration is checked
3. If token expires within 2 minutes, it's automatically refreshed
4. New tokens are persisted to disk
5. Session update callback is called

### 3. Multi-Account Support

Store and manage multiple accounts:

```rust
// Store multiple sessions
session_manager.store_session(alice_session).await?;
session_manager.store_session(bob_session).await?;

// List all stored sessions
let dids = session_manager.list_sessions().await?;
for did in dids {
    println!("Found session for: {}", did);
}

// Load a specific session
let session = session_manager.load_session("did:plc:xyz123").await?;
```

### 4. Session Event Callbacks

React to session lifecycle events:

```rust
use atproto::types::AtpSessionEvent;
use std::sync::Arc;

session_manager.on_session_event(Arc::new(|event, session| {
    match event {
        AtpSessionEvent::Create => {
            if let Some(s) = session {
                println!("Session created for {}", s.handle);
            }
        }
        AtpSessionEvent::Update => {
            println!("Session updated (token refreshed)");
        }
        AtpSessionEvent::Delete => {
            println!("Session deleted");
        }
        AtpSessionEvent::Expired => {
            println!("Session expired");
        }
        _ => {}
    }
}));
```

**Available events:**
- `Create` - Session created or loaded
- `Update` - Session updated (token refresh)
- `Delete` - Session cleared
- `Expired` - Session expired
- `CreateFailed` - Session creation failed
- `NetworkError` - Network error during session operation

### 5. Session Clearing

Clear the current session:

```rust
// Clear session (deletes from memory and disk)
session_manager.clear_session().await?;
```

## Complete Example

```rust
use atproto::session_manager::PersistentSessionManager;
use atproto::types::{AtpSessionData, AtpSessionEvent};
use atproto::client::com::atproto::server::create_session;
use atproto::xrpc::XrpcClientImpl;
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Create session manager
    let mut session_manager = PersistentSessionManager::new(
        PathBuf::from("./sessions"),
        "https://bsky.social".to_string()
    );

    // 2. Set up event callback
    session_manager.on_session_event(Arc::new(|event, session| {
        match event {
            AtpSessionEvent::Create => println!("✓ Session created"),
            AtpSessionEvent::Update => println!("✓ Token refreshed"),
            AtpSessionEvent::Delete => println!("✓ Session deleted"),
            _ => {}
        }
    }));

    // 3. Login
    let client = XrpcClientImpl::new("https://bsky.social".to_string());
    let input = create_session::Input {
        identifier: "alice.bsky.social".to_string(),
        password: "app-password".to_string(),
        auth_factor_token: None,
        allow_takendown: None,
    };

    let response = create_session::create_session(&client, input).await?;

    // 4. Store session
    let session_data = AtpSessionData {
        refresh_jwt: response.data.refresh_jwt,
        access_jwt: response.data.access_jwt,
        handle: response.data.handle,
        did: response.data.did.to_string(),
        email: response.data.email,
        email_confirmed: response.data.email_confirmed,
        email_auth_factor: response.data.email_auth_factor,
        active: response.data.active.unwrap_or(true),
        status: response.data.status,
    };

    session_manager.store_session(session_data.clone()).await?;

    // 5. On app restart, load the session
    let loaded = session_manager.load_session(&session_data.did).await?;
    if let Some(session) = loaded {
        println!("Restored session for {}", session.handle);
    }

    // 6. Clear session on logout
    session_manager.clear_session().await?;

    Ok(())
}
```

## Session File Format

Sessions are stored as JSON files:

```json
{
  "refreshJwt": "eyJhbGci...",
  "accessJwt": "eyJhbGci...",
  "handle": "alice.bsky.social",
  "did": "did:plc:xyz123abc",
  "email": "alice@example.com",
  "emailConfirmed": true,
  "emailAuthFactor": false,
  "active": true,
  "status": null
}
```

## JWT Token Refresh

The SDK automatically parses JWT tokens to determine expiration:

```rust
// JWT structure: header.payload.signature
// Payload contains: { "exp": 1234567890, ... }

// Tokens are refreshed when:
// - current_time > (exp - 120 seconds)
// - This gives a 2-minute buffer before expiration
```

## Thread Safety

The `PersistentSessionManager` is thread-safe:

```rust
use std::sync::Arc;

let manager = Arc::new(PersistentSessionManager::new(
    PathBuf::from("./sessions"),
    "https://bsky.social".to_string()
));

// Can be cloned and shared across threads
let manager_clone = Arc::clone(&manager);
tokio::spawn(async move {
    manager_clone.refresh_token().await
});
```

## Integration with Agent

The Agent can use a PersistentSessionManager:

```rust
use atproto::agent::Agent;
use atproto::session_manager::PersistentSessionManager;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

let agent = Agent::new("https://bsky.social".to_string());

// Create persistent session manager
let session_manager = PersistentSessionManager::new(
    PathBuf::from("./sessions"),
    "https://bsky.social".to_string()
);

// Note: Currently Agent uses XRPC client directly
// Session persistence can be used alongside Agent's login method
// to store credentials for app restart
```

## Error Handling

```rust
use atproto::session_manager::SessionError;

match session_manager.load_session("did:plc:xyz").await {
    Ok(Some(session)) => println!("Loaded: {}", session.handle),
    Ok(None) => println!("No session found"),
    Err(SessionError::NoSession) => println!("Not authenticated"),
    Err(SessionError::Session(msg)) => println!("Error: {}", msg),
    Err(e) => println!("Other error: {}", e),
}
```

## Best Practices

### 1. Session Storage Location

```rust
// Use a platform-appropriate directory
#[cfg(target_os = "macos")]
let session_dir = PathBuf::from(env::var("HOME")?)
    .join("Library/Application Support/YourApp/sessions");

#[cfg(target_os = "linux")]
let session_dir = PathBuf::from(env::var("HOME")?)
    .join(".config/yourapp/sessions");

#[cfg(target_os = "windows")]
let session_dir = PathBuf::from(env::var("APPDATA")?)
    .join("YourApp/sessions");
```

### 2. Secure Token Storage

Session files contain sensitive tokens. Consider:

```rust
// Set restrictive file permissions (Unix)
#[cfg(unix)]
{
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let mut perms = fs::metadata(&session_path)?.permissions();
    perms.set_mode(0o600); // Owner read/write only
    fs::set_permissions(&session_path, perms)?;
}

// Or use platform keychain (future enhancement)
```

### 3. Handling Multiple Accounts

```rust
// Load all sessions on startup
let dids = session_manager.list_sessions().await?;

// Let user choose which account to use
for (i, did) in dids.iter().enumerate() {
    if let Ok(Some(session)) = session_manager.load_session(did).await {
        println!("{}. {} ({})", i+1, session.handle, did);
    }
}

// Switch accounts by loading different sessions
let selected_did = user_selected_did;
session_manager.load_session(&selected_did).await?;
```

### 4. Token Refresh Strategy

```rust
// Default: Auto-refresh 2 minutes before expiration
// This works well for most apps

// For long-running apps, keep auto-refresh enabled
session_manager.enable_auto_refresh();

// For short-lived scripts, disable to avoid unnecessary refreshes
session_manager.disable_auto_refresh();
```

## Comparison with TypeScript SDK

| Feature | TypeScript | Rust | Status |
|---------|-----------|------|--------|
| Session persistence | ✅ | ✅ | Complete |
| Automatic token refresh | ✅ | ✅ | Complete |
| Session callbacks | ✅ | ✅ | Complete |
| Multi-account support | ✅ | ✅ | Complete |
| File-based storage | ✅ | ✅ | Complete |
| Custom storage backends | ✅ | ⚠️ | Partial (trait extensible) |

## Future Enhancements

Possible future improvements:

- [ ] Custom storage backends (database, keychain, etc.)
- [ ] Encrypted session storage
- [ ] Session migration tools
- [ ] Automatic session cleanup (expired sessions)
- [ ] Session sharing across processes
- [ ] OAuth integration with session persistence

## See Also

- [examples/persistent_session.rs](examples/persistent_session.rs) - Complete working example
- [src/session_manager.rs](src/session_manager.rs) - Implementation details
- [src/types.rs](src/types.rs) - AtpSessionData structure

## Conclusion

Session persistence is now **fully implemented** in the Rust SDK, matching the TypeScript SDK's functionality. This enables:
- ✅ **Automatic session restore** on app restart
- ✅ **No re-login required** for returning users
- ✅ **Seamless token refresh** before expiration
- ✅ **Multi-account** management
- ✅ **Production-ready** session handling

The implementation is complete and ready for use in production applications!
