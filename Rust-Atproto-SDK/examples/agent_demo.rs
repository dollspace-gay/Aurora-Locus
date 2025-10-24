//! Comprehensive Agent API demonstration
//!
//! This example demonstrates the full Agent API including:
//! - Session management (login/logout/refresh)
//! - Namespace access (com, app, chat, tools)
//! - Direct API calls through namespaces
//! - Convenience methods (post, follow, like, etc.)
//!
//! Run with:
//! ```
//! cargo run --example agent_demo
//! ```

use atproto::agent::Agent;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== ATProto Rust SDK - Agent API Demo ===\n");

    // ============================================================================
    // 1. Create an Agent
    // ============================================================================

    println!("1. Creating Agent...");
    let agent = Agent::new("https://bsky.social".to_string());
    println!("   Service: {}", agent.service());
    println!("   Authenticated: {}\n", agent.is_authenticated());

    // ============================================================================
    // 2. Namespace Access
    // ============================================================================

    println!("2. Namespace Access:");
    println!("   Available namespaces:");
    println!("   - agent.com()   -> com.atproto.* APIs");
    println!("   - agent.app()   -> app.bsky.* APIs");
    println!("   - agent.chat()  -> chat.bsky.* APIs");
    println!("   - agent.tools() -> tools.ozone.* APIs\n");

    // Access nested namespaces
    println!("   Accessing nested namespaces:");
    let _server_ns = agent.com().atproto().server();
    println!("   ✓ agent.com().atproto().server()");

    let _feed_ns = agent.app().bsky().feed();
    println!("   ✓ agent.app().bsky().feed()");

    let _convo_ns = agent.chat().bsky().convo();
    println!("   ✓ agent.chat().bsky().convo()");

    let _moderation_ns = agent.tools().ozone().moderation();
    println!("   ✓ agent.tools().ozone().moderation()\n");

    // ============================================================================
    // 3. Configuration
    // ============================================================================

    println!("3. Agent Configuration:");

    // Configure labelers
    agent.configure_labelers(vec!["did:plc:labeler123".to_string()]);
    println!("   ✓ Configured labelers");

    // Set custom headers
    agent.set_header("X-Custom-Header".to_string(), "custom-value".to_string());
    println!("   ✓ Set custom headers");

    // Configure proxy
    use atproto::types::{AtprotoServiceType, Did, AtprotoProxy};
    let did = Did::new("did:plc:proxy123").unwrap();
    let service = AtprotoServiceType::new_unchecked("atproto_labeler");
    let proxy = AtprotoProxy::new(did, service);
    agent.configure_proxy(Some(proxy));
    println!("   ✓ Configured proxy\n");

    // ============================================================================
    // 4. Session Management (Demonstration)
    // ============================================================================

    println!("4. Session Management:");
    println!("   NOTE: These methods require valid credentials.");
    println!("   Available authentication methods:");
    println!("   - agent.login(identifier, password)");
    println!("   - agent.resume_session(access_jwt, refresh_jwt, did, handle)");
    println!("   - agent.logout()");
    println!("   - agent.refresh_session()\n");

    // Example of checking authentication
    if agent.is_authenticated() {
        println!("   Current DID: {}", agent.did().unwrap());
    } else {
        println!("   Not authenticated (expected)");
    }
    println!();

    // ============================================================================
    // 5. API Calls Through Namespaces
    // ============================================================================

    println!("5. API Calls Through Namespaces:");
    println!("   Example patterns:");
    println!();

    println!("   // Get timeline");
    println!("   use atproto::client::app::bsky::feed::get_timeline;");
    println!("   let params = get_timeline::QueryParams {{");
    println!("       algorithm: None,");
    println!("       limit: Some(50),");
    println!("       cursor: None,");
    println!("   }};");
    println!("   let response = get_timeline::get_timeline(&*agent.xrpc(), params).await?;");
    println!();

    println!("   // Create a post");
    println!("   use atproto::client::com::atproto::repo::create_record;");
    println!("   let input = create_record::Input {{ /* ... */ }};");
    println!("   let response = create_record::create_record(&*agent.xrpc(), input).await?;");
    println!();

    println!("   // Resolve a handle");
    println!("   use atproto::client::com::atproto::identity::resolve_handle;");
    println!("   let params = resolve_handle::QueryParams {{");
    println!("       handle: \"alice.bsky.social\".to_string(),");
    println!("   }};");
    println!("   let response = resolve_handle::resolve_handle(&*agent.xrpc(), params).await?;");
    println!();

    // ============================================================================
    // 6. Convenience Methods
    // ============================================================================

    println!("6. Convenience Methods:");
    println!("   The Agent provides high-level convenience methods:");
    println!();

    println!("   // Posting");
    println!("   agent.post(\"Hello world! #rustlang\").await?;");
    println!("   agent.post_with_images(\"Check this out!\", images).await?;");
    println!();

    println!("   // Social actions");
    println!("   agent.follow(\"did:plc:...\").await?;");
    println!("   agent.like(\"at://...\", \"cid\").await?;");
    println!("   agent.repost(\"at://...\", \"cid\").await?;");
    println!();

    println!("   // Content operations");
    println!("   agent.delete_record(\"at://...\").await?;");
    println!("   agent.get_timeline(Some(50)).await?;");
    println!("   agent.get_profile(\"alice.bsky.social\").await?;");
    println!();

    println!("   // Handle resolution");
    println!("   agent.resolve_handle(\"alice.bsky.social\").await?;");
    println!();

    println!("   // Blob upload");
    println!("   agent.upload_blob(image_data, \"image/jpeg\").await?;");
    println!();

    // ============================================================================
    // 7. Advanced Features
    // ============================================================================

    println!("7. Advanced Features:");
    println!();

    // Clone agent with different config
    println!("   // Clone agent with proxy");
    println!("   let proxied_agent = agent.with_proxy(");
    println!("       AtprotoServiceType::new_unchecked(\"atproto_labeler\"),");
    println!("       \"did:plc:labeler\".to_string()");
    println!("   );");
    println!();

    // Direct XRPC access
    println!("   // Direct XRPC client access for custom requests");
    println!("   let xrpc_client = agent.xrpc();");
    println!("   // Use xrpc_client for low-level requests");
    println!();

    // ============================================================================
    // Summary
    // ============================================================================

    println!("=== Summary ===");
    println!();
    println!("The Agent provides three levels of API access:");
    println!();
    println!("1. High-level convenience methods:");
    println!("   agent.post(), agent.follow(), agent.like(), etc.");
    println!();
    println!("2. Namespace-organized API endpoints:");
    println!("   agent.app().bsky().feed().get_timeline()");
    println!("   agent.com().atproto().server().create_session()");
    println!();
    println!("3. Direct XRPC client for custom requests:");
    println!("   agent.xrpc().request(custom_request).await");
    println!();
    println!("All 292+ auto-generated API endpoints are accessible!");
    println!();
    println!("For a working example with authentication, see:");
    println!("  - examples/simple_bot.rs");
    println!("  - examples/persistent_session.rs");
    println!();

    Ok(())
}
