//! Persistent Session Example
//!
//! This example demonstrates how to use the PersistentSessionManager
//! for automatic session persistence and token refresh.
//!
//! Run with:
//! ```bash
//! cargo run --example persistent_session
//! ```

use atproto::session_manager::{PersistentSessionManager, SessionCallback};
use atproto::types::{AtpSessionData, AtpSessionEvent};
use atproto::client::com::atproto::server::create_session;
use atproto::xrpc::{XrpcClient, XrpcClientImpl};
use std::path::PathBuf;
use std::sync::Arc;
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🦀 Rust ATProto SDK - Persistent Session Example\n");

    // Get credentials from environment variables
    let handle = env::var("BSKY_HANDLE")
        .unwrap_or_else(|_| "your-handle.bsky.social".to_string());
    let password = env::var("BSKY_PASSWORD")
        .unwrap_or_else(|_| "your-app-password".to_string());

    // Example 1: Create a persistent session manager
    println!("📁 Example 1: Creating Persistent Session Manager\n");

    let mut session_manager = PersistentSessionManager::new(
        PathBuf::from("./sessions"),
        "https://bsky.social".to_string()
    );

    // Set up session event callback
    session_manager.on_session_event(Arc::new(|event, session| {
        match event {
            AtpSessionEvent::Create => {
                if let Some(s) = session {
                    println!("  ✓ Session created for {}", s.handle);
                }
            }
            AtpSessionEvent::Update => {
                println!("  ✓ Session updated (token refreshed)");
            }
            AtpSessionEvent::Delete => {
                println!("  ✓ Session deleted");
            }
            _ => {}
        }
    }));

    println!("✓ Session manager created with auto-refresh enabled\n");

    // Example 2: Login and create session
    println!("📝 Example 2: Login and Store Session\n");

    if handle == "your-handle.bsky.social" {
        println!("⚠️  Skipping login - no credentials provided");
        println!("   Set BSKY_HANDLE and BSKY_PASSWORD environment variables to test");
        print_session_info(&session_manager).await?;
        return Ok(());
    }

    // Create XRPC client for login
    let client = XrpcClientImpl::new("https://bsky.social".to_string());

    let input = create_session::Input {
        identifier: handle.clone(),
        password: password.clone(),
        auth_factor_token: None,
        allow_takendown: None,
    };

    println!("  Logging in as {}...", handle);
    let response = create_session::create_session(&client, input).await?;
    println!("  ✓ Login successful!");

    // Convert response to AtpSessionData
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

    // Store session
    session_manager.store_session(session_data.clone()).await?;
    println!("  ✓ Session persisted to disk\n");

    // Example 3: List stored sessions
    println!("📋 Example 3: List Stored Sessions\n");
    let sessions = session_manager.list_sessions().await?;
    println!("  Found {} stored session(s):", sessions.len());
    for did in &sessions {
        println!("    - {}", did);
    }
    println!();

    // Example 4: Load session from disk
    println!("💾 Example 4: Load Session from Disk\n");

    let loaded_session = session_manager.load_session(&session_data.did).await?;
    if let Some(session) = loaded_session {
        println!("  ✓ Session loaded successfully!");
        println!("    Handle: {}", session.handle);
        println!("    DID: {}", session.did);
        println!("    Active: {}", session.active);
    } else {
        println!("  ✗ No session found");
    }
    println!();

    // Example 5: Automatic token refresh
    println!("🔄 Example 5: Token Refresh\n");
    println!("  Auto-refresh is enabled by default");
    println!("  Tokens will be automatically refreshed when they expire\n");

    // Manually trigger a refresh (normally happens automatically)
    println!("  Manually refreshing token...");
    match session_manager.refresh_token().await {
        Ok(_) => println!("  ✓ Token refreshed successfully!\n"),
        Err(e) => println!("  ⚠️  Refresh failed (may not be needed yet): {}\n", e),
    }

    // Example 6: Multi-account support
    println!("👥 Example 6: Multi-Account Support\n");
    println!("  Each DID gets its own session file:");
    println!("  - sessions/did_plc_xyz123.json");
    println!("  - sessions/did_plc_abc456.json");
    println!("  You can switch between accounts by loading different sessions\n");

    // Example 7: Session callbacks
    println!("📢 Example 7: Session Event Callbacks\n");
    println!("  Callbacks were already demonstrated above!");
    println!("  Events: Create, Update, Delete, Expired, CreateFailed, NetworkError\n");

    // Example 8: Clear session
    println!("🗑️  Example 8: Clear Session\n");
    println!("  Clearing session (this will delete the session file)...");
    session_manager.clear_session().await?;
    println!("  ✓ Session cleared\n");

    // Verify session was deleted
    let sessions_after = session_manager.list_sessions().await?;
    println!("  Remaining sessions: {}", sessions_after.len());

    println!("\n✨ Session persistence demo complete!");
    println!("\n💡 Key Features:");
    println!("  ✓ Automatic file-based storage");
    println!("  ✓ Automatic token refresh before expiration");
    println!("  ✓ Multi-account support");
    println!("  ✓ Session event callbacks");
    println!("  ✓ Thread-safe with Arc<RwLock>");

    Ok(())
}

async fn print_session_info(manager: &PersistentSessionManager) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n📊 Session Information:\n");

    let sessions = manager.list_sessions().await?;
    if sessions.is_empty() {
        println!("  No stored sessions found");
    } else {
        println!("  {} stored session(s):", sessions.len());
        for did in sessions {
            if let Ok(Some(session)) = manager.load_session(&did).await {
                println!("    Handle: {}", session.handle);
                println!("    DID: {}", session.did);
                println!("    Active: {}", session.active);
                println!();
            }
        }
    }

    Ok(())
}
