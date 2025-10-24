//! OAuth 2.0 with PKCE Flow Demonstration
//!
//! This example demonstrates the complete OAuth authorization code flow
//! with PKCE (Proof Key for Code Exchange) and DPoP (Demonstrating Proof of Possession).
//!
//! ## Prerequisites
//!
//! 1. Host your client metadata JSON at a public URL
//! 2. Configure a redirect URI (callback URL)
//! 3. Have a web server to handle the OAuth callback
//!
//! ## Flow
//!
//! 1. Generate PKCE parameters (code_verifier and code_challenge)
//! 2. Build authorization URL and redirect user
//! 3. User authorizes on the authorization server
//! 4. Handle callback and extract authorization code
//! 5. Exchange code for tokens using code_verifier
//! 6. Use tokens to make authenticated requests
//!
//! ## Running this Example
//!
//! This is a demonstration of the OAuth flow. In a real application:
//! - You would run a web server to handle the callback
//! - You would store the PKCE code_verifier securely during the flow
//! - You would redirect the user's browser to the authorization URL
//! - You would extract the authorization code from the callback URL
//!
//! ```bash
//! cargo run --example oauth_demo
//! ```

use atproto::oauth::{OAuthClient, PkceParams};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== OAuth 2.0 with PKCE Flow Demonstration ===\n");

    // Step 1: Configure OAuth client
    println!("Step 1: Configure OAuth Client");
    println!("================================");

    let client_id = "https://example.com/client-metadata.json".to_string();
    let redirect_uri = "https://example.com/callback".to_string();

    let client = OAuthClient::new(client_id.clone(), redirect_uri.clone())?;

    println!("Client ID: {}", client_id);
    println!("Redirect URI: {}", redirect_uri);
    println!();

    // Step 2: Generate PKCE parameters
    println!("Step 2: Generate PKCE Parameters");
    println!("==================================");

    let pkce = PkceParams::generate();

    println!("Code Verifier Length: {} chars", pkce.code_verifier.len());
    println!("Code Verifier (first 20 chars): {}...", &pkce.code_verifier[..20]);
    println!("Code Challenge: {}", pkce.code_challenge);
    println!("Code Challenge Method: {}", pkce.code_challenge_method);
    println!();

    println!("⚠️  IMPORTANT: Store code_verifier securely!");
    println!("   You will need it to exchange the authorization code for tokens.");
    println!();

    // Step 3: Build authorization URL
    println!("Step 3: Build Authorization URL");
    println!("================================");

    let pds_url = "https://bsky.social";
    let handle = "user.bsky.social";

    match client.build_authorization_url(pds_url, handle, &pkce).await {
        Ok(auth_url) => {
            println!("Authorization URL:");
            println!("{}", auth_url);
            println!();
            println!("Next steps:");
            println!("1. Redirect the user to this URL");
            println!("2. User will authorize your application");
            println!("3. User will be redirected back to: {}", redirect_uri);
            println!("4. Extract the 'code' parameter from the callback URL");
        }
        Err(e) => {
            println!("Failed to build authorization URL: {}", e);
            println!("This is expected in this demo (no actual authorization server)");
        }
    }
    println!();

    // Step 4: Token Exchange (Simulated)
    println!("Step 4: Token Exchange");
    println!("======================");
    println!("After receiving the callback with an authorization code:");
    println!();
    println!("```rust");
    println!("let session = client.exchange_code(");
    println!("    &authorization_code,  // From callback URL");
    println!("    &pkce.code_verifier,  // Stored from Step 2");
    println!("    \"https://bsky.social/oauth/token\",");
    println!(").await?;");
    println!("```");
    println!();

    // Step 5: DPoP Demonstration
    println!("Step 5: DPoP (Demonstrating Proof of Possession)");
    println!("=================================================");

    let dpop = client.get_dpop();
    let dpop_proof = dpop.generate_proof(
        "POST",
        "https://bsky.social/xrpc/com.atproto.server.createSession",
    )?;

    println!("DPoP Proof JWT generated:");
    println!("Header: Contains RSA public key (JWK)");
    println!("Claims: jti, htm, htu, iat, exp");
    println!("Signature: RSA-SHA256 signature");
    println!();
    println!("DPoP Proof (first 100 chars):");
    println!("{}...", &dpop_proof[..100.min(dpop_proof.len())]);
    println!();

    // Step 6: Using the Session
    println!("Step 6: Using OAuth Session");
    println!("============================");
    println!("Once you have a session, you can:");
    println!("1. Convert it to AtpSessionData:");
    println!("   let atp_session = oauth_session.to_atp_session_data();");
    println!();
    println!("2. Use it with the Agent:");
    println!("   let agent = Agent::new(\"https://bsky.social\".to_string());");
    println!("   agent.resume_session(atp_session).await?;");
    println!();
    println!("3. Make authenticated API calls:");
    println!("   let timeline = agent.get_timeline(Some(50)).await?;");
    println!();

    // Step 7: Token Refresh
    println!("Step 7: Token Refresh");
    println!("=====================");
    println!("When the access token expires, refresh it:");
    println!();
    println!("```rust");
    println!("let new_session = client.refresh_token(");
    println!("    &refresh_token,");
    println!("    \"https://bsky.social/oauth/token\",");
    println!(").await?;");
    println!("```");
    println!();

    // Security Notes
    println!("=== Security Notes ===");
    println!();
    println!("✅ PKCE: Protects against authorization code interception");
    println!("   - code_verifier is kept secret on the client");
    println!("   - code_challenge is sent in authorization request");
    println!("   - Server verifies: SHA256(verifier) == stored challenge");
    println!();
    println!("✅ DPoP: Binds access tokens to client's private key");
    println!("   - Each request includes a DPoP proof JWT");
    println!("   - Proof is signed with client's RSA private key");
    println!("   - Prevents token theft and replay attacks");
    println!();
    println!("✅ State Parameter: Protects against CSRF attacks");
    println!("   - Random state value sent in authorization request");
    println!("   - Must match in callback to prevent CSRF");
    println!();

    // Client Metadata Example
    println!("=== Client Metadata Example ===");
    println!();
    println!("Host this JSON at your client_id URL:");
    println!();
    println!("{}", serde_json::to_string_pretty(client.get_metadata())?);
    println!();

    println!("=== Demo Complete ===");
    println!();
    println!("For a complete implementation:");
    println!("1. Set up a web server with OAuth callback handling");
    println!("2. Host your client metadata at the client_id URL");
    println!("3. Implement state parameter validation");
    println!("4. Securely store PKCE code_verifier during the flow");
    println!("5. Store refresh tokens securely for long-lived sessions");

    Ok(())
}
