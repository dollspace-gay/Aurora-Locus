//! Simple Bluesky Bot Example
//!
//! This example demonstrates how to use the ATProto Rust SDK to:
//! - Login to a Bluesky account
//! - Create a post
//! - Get your timeline
//! - Get a profile
//!
//! Run with:
//! ```bash
//! cargo run --example simple_bot
//! ```

use atproto::agent::Agent;
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🦀 Rust ATProto SDK - Simple Bot Example\n");

    // Get credentials from environment variables
    let handle = env::var("BSKY_HANDLE")
        .unwrap_or_else(|_| "your-handle.bsky.social".to_string());
    let password = env::var("BSKY_PASSWORD")
        .unwrap_or_else(|_| "your-app-password".to_string());

    // Create agent
    let agent = Agent::new("https://bsky.social".to_string());
    println!("✓ Created agent for {}", agent.service());

    // Login
    println!("\n📝 Logging in as {}...", handle);
    match agent.login(&handle, &password).await {
        Ok(_) => println!("✓ Logged in successfully!"),
        Err(e) => {
            eprintln!("✗ Login failed: {}", e);
            eprintln!("\nTo use this example:");
            eprintln!("  export BSKY_HANDLE=your-handle.bsky.social");
            eprintln!("  export BSKY_PASSWORD=your-app-password");
            return Err(e.into());
        }
    }

    if let Some(did) = agent.did() {
        println!("✓ Authenticated as DID: {}", did);
    }

    // Create a post
    println!("\n📮 Creating a post...");
    match agent.post("Hello from the Rust ATProto SDK! 🦀").await {
        Ok(uri) => println!("✓ Posted! URI: {}", uri),
        Err(e) => eprintln!("✗ Failed to post: {}", e),
    }

    // Get timeline
    println!("\n📰 Fetching timeline (5 posts)...");
    match agent.get_timeline(Some(5)).await {
        Ok(timeline) => {
            println!("✓ Timeline fetched!");
            if let Some(feed) = timeline.get("feed").and_then(|f| f.as_array()) {
                for (i, post) in feed.iter().enumerate() {
                    if let (Some(author), Some(text)) = (
                        post.get("post")
                            .and_then(|p| p.get("author"))
                            .and_then(|a| a.get("handle"))
                            .and_then(|h| h.as_str()),
                        post.get("post")
                            .and_then(|p| p.get("record"))
                            .and_then(|r| r.get("text"))
                            .and_then(|t| t.as_str()),
                    ) {
                        println!("  {}. @{}: {}", i + 1, author, text);
                    }
                }
            }
        }
        Err(e) => eprintln!("✗ Failed to get timeline: {}", e),
    }

    // Get a profile
    println!("\n👤 Fetching profile for bsky.app...");
    match agent.get_profile("bsky.app").await {
        Ok(profile) => {
            if let (Some(handle), Some(display_name)) = (
                profile.get("handle").and_then(|h| h.as_str()),
                profile.get("displayName").and_then(|d| d.as_str()),
            ) {
                println!("✓ Profile: {} (@{})", display_name, handle);
                if let Some(description) = profile.get("description").and_then(|d| d.as_str()) {
                    println!("  Bio: {}", description);
                }
            }
        }
        Err(e) => eprintln!("✗ Failed to get profile: {}", e),
    }

    println!("\n✨ Done!");

    Ok(())
}
