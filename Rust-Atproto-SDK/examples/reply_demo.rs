//! Reply Demo - Demonstrates creating reply posts in threads
//!
//! This example shows how to:
//! - Create original posts
//! - Reply to posts
//! - Reply to replies (threading)
//! - Reply with images
//! - Use automatic facet detection in replies
//!
//! Note: This is a demonstration. To run with real credentials:
//! 1. Replace the login credentials
//! 2. Uncomment the actual API calls
//! 3. Handle the returned URIs/CIDs properly

use atproto::agent::Agent;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== ATProto Reply Demo ===\n");

    // Create agent
    let agent = Agent::new("https://bsky.social".to_string());
    println!("✓ Created agent for https://bsky.social\n");

    // ============================================================================
    // Example 1: Basic Reply to a Post
    // ============================================================================
    println!("Example 1: Basic Reply to a Post");
    println!("─────────────────────────────────");

    // In a real scenario, you would:
    // 1. Login
    // agent.login("alice.bsky.social", "app-password").await?;

    // 2. Create an original post
    // let original_uri = agent.post("What's everyone's favorite programming language?").await?;
    // println!("Original post: {}", original_uri);

    // 3. Get the CID from the post (you'd need to fetch the post details)
    // For demo purposes, we'll use placeholder values
    let original_uri = "at://did:plc:abc123/app.bsky.feed.post/xyz789";
    let original_cid = "bafyreiabc123def456";

    // 4. Reply to the post
    println!("\nReplying to post...");
    println!("  Parent: {}", original_uri);
    println!("  Root:   {}", original_uri);
    println!("  Text:   \"Rust! It has the best type system and memory safety.\"");

    // Actual call (commented out for demo):
    // let reply_uri = agent.post_reply(
    //     "Rust! It has the best type system and memory safety.",
    //     original_uri,
    //     original_cid,
    //     original_uri,  // Root is same as parent for direct reply
    //     original_cid,
    // ).await?;
    // println!("Reply created: {}", reply_uri);

    println!("✓ Reply would be created with proper threading\n");

    // ============================================================================
    // Example 2: Reply to a Reply (Nested Threading)
    // ============================================================================
    println!("Example 2: Reply to a Reply (Nested Threading)");
    println!("───────────────────────────────────────────────");

    // Scenario: Someone replied to the original post, and we want to reply to their reply
    let first_reply_uri = "at://did:plc:def456/app.bsky.feed.post/abc123";
    let first_reply_cid = "bafyreifirst789xyz";

    println!("\nReplying to a reply...");
    println!("  Parent: {} (the first reply)", first_reply_uri);
    println!("  Root:   {} (original post)", original_uri);
    println!("  Text:   \"@bob.bsky.social I agree! The ownership system is brilliant.\"");

    // Actual call (commented out for demo):
    // let nested_reply_uri = agent.post_reply(
    //     "@bob.bsky.social I agree! The ownership system is brilliant.",
    //     first_reply_uri,    // Parent is the reply we're responding to
    //     first_reply_cid,
    //     original_uri,       // Root is still the original post
    //     original_cid,
    // ).await?;
    // println!("Nested reply created: {}", nested_reply_uri);

    println!("✓ Nested reply maintains thread structure\n");

    // ============================================================================
    // Example 3: Reply with Images
    // ============================================================================
    println!("Example 3: Reply with Images");
    println!("────────────────────────────");

    // Create a simple test image (1x1 red PNG)
    let red_png: Vec<u8> = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
        0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1 dimensions
        0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, // IHDR data
        0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, // IDAT chunk
        0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, // Pixel data (red)
        0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB4, // CRC
        0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, // IEND chunk
        0xAE, 0x42, 0x60, 0x82,
    ];

    println!("\nReplying with image...");
    println!("  Parent: {}", original_uri);
    println!("  Root:   {}", original_uri);
    println!("  Text:   \"Here's my setup! #rustlang\"");
    println!("  Images: 1 image with alt text");

    // Actual call (commented out for demo):
    // let images = vec![(red_png, "My Rust development setup".to_string())];
    // let image_reply_uri = agent.post_reply_with_images(
    //     "Here's my setup! #rustlang",
    //     images,
    //     original_uri,
    //     original_cid,
    //     original_uri,
    //     original_cid,
    // ).await?;
    // println!("Reply with image created: {}", image_reply_uri);

    println!("✓ Reply with image would be created\n");

    // ============================================================================
    // Example 4: Reply with Mentions and Links
    // ============================================================================
    println!("Example 4: Reply with Mentions and Links");
    println!("─────────────────────────────────────────");

    println!("\nReplying with rich text...");
    println!("  Parent: {}", original_uri);
    println!("  Root:   {}", original_uri);
    println!("  Text:   \"@alice.bsky.social Check out https://www.rust-lang.org #rustlang\"");
    println!();
    println!("Features:");
    println!("  ✓ Automatic mention detection (@alice.bsky.social)");
    println!("  ✓ Automatic link detection (https://www.rust-lang.org)");
    println!("  ✓ Automatic hashtag detection (#rustlang)");
    println!("  ✓ Mention resolution (handle → DID)");

    // Actual call (commented out for demo):
    // let rich_reply_uri = agent.post_reply(
    //     "@alice.bsky.social Check out https://www.rust-lang.org #rustlang",
    //     original_uri,
    //     original_cid,
    //     original_uri,
    //     original_cid,
    // ).await?;
    // println!("Rich text reply created: {}", rich_reply_uri);

    println!("✓ All facets would be automatically detected and created\n");

    // ============================================================================
    // Example 5: Understanding Thread Structure
    // ============================================================================
    println!("Example 5: Understanding Thread Structure");
    println!("──────────────────────────────────────────");
    println!();
    println!("Thread hierarchy:");
    println!("  [Original Post] ← root");
    println!("    ├─ [Reply 1] ← parent=original, root=original");
    println!("    │   └─ [Reply to Reply 1] ← parent=reply1, root=original");
    println!("    └─ [Reply 2] ← parent=original, root=original");
    println!();
    println!("Key concepts:");
    println!("  • root  - The first post in the thread");
    println!("  • parent - The immediate post being replied to");
    println!();
    println!("Direct reply to original:");
    println!("  parent = root = original post");
    println!();
    println!("Reply to a reply:");
    println!("  parent = the reply you're responding to");
    println!("  root   = the original post that started the thread");

    // ============================================================================
    // Example 6: Working Example Structure
    // ============================================================================
    println!("\n\nExample 6: Complete Working Flow");
    println!("─────────────────────────────────");
    println!();
    println!("To use this in a real application:");
    println!();
    println!("1. Login:");
    println!("   let agent = Agent::new(\"https://bsky.social\".to_string());");
    println!("   agent.login(\"your-handle.bsky.social\", \"your-app-password\").await?;");
    println!();
    println!("2. Create original post:");
    println!("   let uri = agent.post(\"Hello world!\").await?;");
    println!();
    println!("3. Extract URI and CID:");
    println!("   // URI is returned directly");
    println!("   // To get CID, use get_posts() to fetch the post details");
    println!("   // Or parse from the create_record response");
    println!();
    println!("4. Create reply:");
    println!("   let reply_uri = agent.post_reply(");
    println!("       \"Great post!\",");
    println!("       &uri,      // parent URI");
    println!("       &cid,      // parent CID");
    println!("       &uri,      // root URI (same for direct reply)");
    println!("       &cid,      // root CID (same for direct reply)");
    println!("   ).await?;");
    println!();
    println!("5. Create reply with images:");
    println!("   let image_data = std::fs::read(\"photo.jpg\")?;");
    println!("   let images = vec![(image_data, \"Alt text\".to_string())];");
    println!("   let reply_uri = agent.post_reply_with_images(");
    println!("       \"Check this out!\",");
    println!("       images,");
    println!("       &parent_uri,");
    println!("       &parent_cid,");
    println!("       &root_uri,");
    println!("       &root_cid,");
    println!("   ).await?;");

    println!("\n✓ Reply demo complete!");
    println!("\nThe Rust ATProto SDK now supports:");
    println!("  ✓ Creating reply posts");
    println!("  ✓ Nested threading (replies to replies)");
    println!("  ✓ Replies with images");
    println!("  ✓ Automatic facet detection in replies");
    println!("  ✓ Type-safe reply reference structures");

    Ok(())
}
