//! Rich Text and Facet Detection Example
//!
//! This example demonstrates how the Rust ATProto SDK automatically detects
//! and creates clickable mentions, links, and hashtags in posts.
//!
//! Run with:
//! ```bash
//! cargo run --example rich_text_demo
//! ```

use atproto::agent::Agent;
use atproto::rich_text::{detect_facets, UnicodeString};
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🦀 Rust ATProto SDK - Rich Text & Facet Detection Example\n");

    // Example 1: Detect facets in text (without posting)
    println!("📝 Example 1: Facet Detection\n");
    demonstrate_facet_detection();

    // Example 2: Post with automatic facet detection
    println!("\n📮 Example 2: Posting with Automatic Facets\n");

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
            eprintln!("\nTo use the posting examples:");
            eprintln!("  export BSKY_HANDLE=your-handle.bsky.social");
            eprintln!("  export BSKY_PASSWORD=your-app-password");
            eprintln!("\nNote: The facet detection demo above works without login!");
            return Ok(());
        }
    }

    if let Some(did) = agent.did() {
        println!("✓ Authenticated as DID: {}", did);
    }

    // Example 3: Post with mention
    println!("\n📌 Example 3: Post with Mention");
    let text = "Hey @bsky.app check out the Rust SDK!";
    println!("  Text: \"{}\"", text);
    match agent.post(text).await {
        Ok(uri) => println!("  ✓ Posted: {}", uri),
        Err(e) => eprintln!("  ✗ Failed: {}", e),
    }

    // Example 4: Post with link
    println!("\n🔗 Example 4: Post with Link");
    let text = "Check out the ATProto docs at https://atproto.com";
    println!("  Text: \"{}\"", text);
    match agent.post(text).await {
        Ok(uri) => println!("  ✓ Posted: {}", uri),
        Err(e) => eprintln!("  ✗ Failed: {}", e),
    }

    // Example 5: Post with hashtag
    println!("\n#️⃣  Example 5: Post with Hashtag");
    let text = "Building with #rustlang is amazing! #atproto #bluesky";
    println!("  Text: \"{}\"", text);
    match agent.post(text).await {
        Ok(uri) => println!("  ✓ Posted: {}", uri),
        Err(e) => eprintln!("  ✗ Failed: {}", e),
    }

    // Example 6: Post with multiple facet types
    println!("\n✨ Example 6: Post with Multiple Facets");
    let text = "Hey @bsky.app! Check https://example.com for more #rustlang content!";
    println!("  Text: \"{}\"", text);
    match agent.post(text).await {
        Ok(uri) => println!("  ✓ Posted: {}", uri),
        Err(e) => eprintln!("  ✗ Failed: {}", e),
    }

    println!("\n✅ Done! All facets are automatically detected and made clickable.");

    Ok(())
}

/// Demonstrate facet detection without posting
fn demonstrate_facet_detection() {
    let examples = vec![
        "Hello @alice.bsky.social!",
        "Check out https://example.com",
        "Love #rustlang #programming",
        "Hey @bob.bsky.social check https://atproto.com #cool",
        "Multiple @alice.bsky.social @bob.bsky.social mentions",
        "Bare domain example.com works too!",
    ];

    for text in examples {
        let unicode_text = UnicodeString::new(text);
        let facets = detect_facets(&unicode_text);

        println!("Text: \"{}\"", text);

        if let Some(facets) = facets {
            println!("  ✓ Detected {} facet(s):", facets.len());
            for (i, facet) in facets.iter().enumerate() {
                let text_slice = &text[facet.index.byte_start..facet.index.byte_end];
                println!("    {}. \"{}\" at [{}-{}]",
                    i + 1,
                    text_slice,
                    facet.index.byte_start,
                    facet.index.byte_end
                );

                for feature in &facet.features {
                    match feature {
                        atproto::rich_text::FacetFeature::Mention { did } => {
                            println!("       → Mention: {} (resolves to DID)", did);
                        }
                        atproto::rich_text::FacetFeature::Link { uri } => {
                            println!("       → Link: {}", uri);
                        }
                        atproto::rich_text::FacetFeature::Tag { tag } => {
                            println!("       → Tag: #{}", tag);
                        }
                    }
                }
            }
        } else {
            println!("  ℹ No facets detected");
        }
        println!();
    }
}
