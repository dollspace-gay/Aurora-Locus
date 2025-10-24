///! Complete OAuth 2.0 Authorization Code Flow Example
///!
///! This example demonstrates the full OAuth flow for authenticating
///! with ATProto services like Bluesky.
///!
///! ## Flow Overview
///!
///! 1. Create OAuth client
///! 2. Generate PKCE parameters
///! 3. Build authorization URL
///! 4. User authorizes in browser
///! 5. Handle callback with authorization code
///! 6. Exchange code for tokens
///! 7. Use tokens to make authenticated API calls
///!
///! ## Running This Example
///!
///! This is a demonstration of the OAuth API. For a full working example,
///! you would need to:
///! - Host client metadata at a public URL
///! - Set up a callback server to receive the authorization code
///! - Handle the browser redirect flow
///!
///! Run with: `cargo run --example oauth_flow`

use atproto::oauth::{OAuthClient, PkceParams};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== ATProto OAuth 2.0 Flow Example ===\n");

    // Step 1: Create OAuth client
    println!("1. Creating OAuth client...");
    let client = OAuthClient::new(
        // Client ID must be a URL where your client metadata is hosted
        "https://example.com/.well-known/client-metadata.json".to_string(),
        // Callback URL where authorization code will be sent
        "https://example.com/oauth/callback".to_string(),
    )?;
    println!("   ✓ Client created\n");

    // Step 2: Generate PKCE parameters
    println!("2. Generating PKCE parameters (RFC 7636)...");
    let pkce = PkceParams::generate();
    println!("   ✓ Code verifier: {} chars", pkce.code_verifier.len());
    println!("   ✓ Code challenge: {}", &pkce.code_challenge[..20]);
    println!("   ✓ Challenge method: {}\n", pkce.code_challenge_method);

    // IMPORTANT: Store the code_verifier securely!
    // You'll need it to exchange the authorization code for tokens
    let stored_verifier = pkce.code_verifier.clone();

    // Step 3: Build authorization URL
    println!("3. Building authorization URL...");
    let auth_url = client
        .build_authorization_url(
            "https://bsky.social",      // PDS URL
            "user.bsky.social",          // User's handle
            &pkce,
        )
        .await?;
    println!("   ✓ Authorization URL generated\n");

    println!("4. Redirect user to authorization URL:");
    println!("   {}\n", auth_url);

    // Step 4: User authorizes in browser
    println!("5. User authorizes the application in their browser...");
    println!("   (In production, you would open this URL in a browser)\n");

    // Step 5: Handle callback
    println!("6. After authorization, user is redirected to callback URL:");
    // Example callback URL (in practice, this comes from the browser redirect)
    let callback_url = "https://example.com/oauth/callback?\
                       code=authorization_code_here&\
                       state=csrf_protection_state";
    println!("   {}\n", callback_url);

    // Step 6: Exchange code for tokens
    println!("7. Exchanging authorization code for tokens...");
    println!("   (This would call the token endpoint)");

    // In a real application:
    // let session = client.handle_callback(
    //     callback_url,
    //     &stored_verifier,
    //     "https://bsky.social/oauth/token",
    // ).await?;

    // Demonstrate the high-level handle_callback API
    println!("\n8. Using handle_callback() for convenience:");
    println!("   This method combines:");
    println!("   • Callback URL parsing");
    println!("   • State validation (CSRF protection)");
    println!("   • Code exchange for tokens");
    println!("   • DPoP proof generation\n");

    // Step 7: Use tokens for API calls
    println!("9. With OAuth session, you can:");
    println!("   • Make authenticated API calls");
    println!("   • Access user's data (with permission)");
    println!("   • Refresh tokens when they expire\n");

    // Example of state management with metadata
    println!("=== Advanced: State with Metadata ===\n");
    let metadata = serde_json::json!({
        "original_page": "/dashboard",
        "theme": "dark",
        "language": "en"
    });

    println!("Generating state with custom metadata...");
    let auth_url_with_metadata = client
        .build_authorization_url_with_metadata(
            "https://bsky.social",
            "user.bsky.social",
            &pkce,
            Some(metadata.clone()),
        )
        .await?;
    println!("✓ State includes metadata that will be returned after auth\n");

    // When handling callback, you can retrieve the metadata:
    // let metadata = client.validate_state_with_metadata(&state);
    // if let Some(meta) = metadata {
    //     // Redirect to original_page, apply theme, etc.
    // }

    println!("=== OAuth Flow Complete ===\n");
    println!("For a complete working example, see:");
    println!("• examples/oauth_server.rs - Local callback server");
    println!("• examples/oauth_cli.rs - CLI OAuth flow\n");

    Ok(())
}
