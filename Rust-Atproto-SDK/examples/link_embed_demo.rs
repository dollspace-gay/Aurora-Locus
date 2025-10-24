//! Link Embed Demo - Demonstrates creating posts with external link cards
//!
//! This example shows how to:
//! - Create posts with link preview cards
//! - Add thumbnails to link cards
//! - Create replies with link embeds
//! - Use automatic facet detection with link embeds
//!
//! Note: This is a demonstration. To run with real credentials:
//! 1. Replace the login credentials
//! 2. Uncomment the actual API calls
//! 3. Have thumbnail images ready

use atproto::agent::Agent;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== ATProto Link Embed Demo ===\n");

    // Create agent
    let agent = Agent::new("https://bsky.social".to_string());
    println!("✓ Created agent for https://bsky.social\n");

    // ============================================================================
    // Example 1: Basic Link Card (No Thumbnail)
    // ============================================================================
    println!("Example 1: Basic Link Card (No Thumbnail)");
    println!("──────────────────────────────────────────");

    println!("\nCreating post with link card...");
    println!("  URL:         https://www.rust-lang.org/");
    println!("  Title:       The Rust Programming Language");
    println!("  Description: A language empowering everyone to build reliable and efficient software.");
    println!("  Thumbnail:   None");

    // Actual call (commented out for demo):
    // let uri = agent.post_with_link_embed(
    //     "Check out Rust! A great systems programming language.",
    //     "https://www.rust-lang.org/",
    //     "The Rust Programming Language",
    //     "A language empowering everyone to build reliable and efficient software.",
    //     None,  // No thumbnail
    // ).await?;
    // println!("\n✓ Post created: {}", uri);

    println!("\n✓ Post would be created with link preview card");

    // ============================================================================
    // Example 2: Link Card with Thumbnail
    // ============================================================================
    println!("\n\nExample 2: Link Card with Thumbnail");
    println!("────────────────────────────────────");

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

    println!("\nCreating post with link card and thumbnail...");
    println!("  URL:         https://blog.rust-lang.org/");
    println!("  Title:       Rust Blog");
    println!("  Description: Official blog of the Rust programming language");
    println!("  Thumbnail:   {} bytes", red_png.len());

    // Actual call (commented out for demo):
    // let uri = agent.post_with_link_card(
    //     "Great article on the Rust blog!",
    //     "https://blog.rust-lang.org/",
    //     "Rust Blog",
    //     "Official blog of the Rust programming language",
    //     red_png,  // Thumbnail image
    // ).await?;
    // println!("\n✓ Post created with thumbnail: {}", uri);

    println!("\n✓ Post would be created with link card and thumbnail");

    // ============================================================================
    // Example 3: Link Card with Mentions and Hashtags
    // ============================================================================
    println!("\n\nExample 3: Link Card with Mentions and Hashtags");
    println!("────────────────────────────────────────────────");

    println!("\nCreating post with link card and rich text...");
    println!("  Text:        @alice.bsky.social check this out! #rustlang");
    println!("  URL:         https://doc.rust-lang.org/");
    println!("  Title:       Rust Documentation");
    println!("  Description: Official documentation for the Rust programming language");

    println!("\nAutomatic features:");
    println!("  ✓ Mention detection (@alice.bsky.social)");
    println!("  ✓ Mention resolution (handle → DID)");
    println!("  ✓ Hashtag detection (#rustlang)");
    println!("  ✓ Link card generation");

    // Actual call (commented out for demo):
    // let uri = agent.post_with_link_embed(
    //     "@alice.bsky.social check this out! #rustlang",
    //     "https://doc.rust-lang.org/",
    //     "Rust Documentation",
    //     "Official documentation for the Rust programming language",
    //     None,
    // ).await?;

    println!("\n✓ All facets would be automatically detected and link card created");

    // ============================================================================
    // Example 4: Reply with Link Card
    // ============================================================================
    println!("\n\nExample 4: Reply with Link Card");
    println!("────────────────────────────────");

    let parent_uri = "at://did:plc:abc/app.bsky.feed.post/xyz";
    let parent_cid = "bafyreiabc";

    println!("\nReplying with link card...");
    println!("  Parent:      {}", parent_uri);
    println!("  Root:        {}", parent_uri);
    println!("  Text:        Here's more info on that topic!");
    println!("  URL:         https://rust-lang.github.io/");
    println!("  Title:       Rust Project Documentation");
    println!("  Description: Community documentation and resources");

    // Actual call (commented out for demo):
    // let reply_uri = agent.reply_with_link_embed(
    //     "Here's more info on that topic!",
    //     "https://rust-lang.github.io/",
    //     "Rust Project Documentation",
    //     "Community documentation and resources",
    //     None,
    //     parent_uri,
    //     parent_cid,
    //     parent_uri,  // Same as parent for direct reply
    //     parent_cid,
    // ).await?;

    println!("\n✓ Reply with link card would be created");

    // ============================================================================
    // Example 5: Multiple Posts with Different Link Cards
    // ============================================================================
    println!("\n\nExample 5: Multiple Posts with Different Link Cards");
    println!("────────────────────────────────────────────────────");

    let links = vec![
        (
            "https://crates.io/",
            "Crates.io",
            "The Rust community's crate registry",
        ),
        (
            "https://play.rust-lang.org/",
            "Rust Playground",
            "Try Rust code in your browser",
        ),
        (
            "https://this-week-in-rust.org/",
            "This Week in Rust",
            "Weekly newsletter about Rust",
        ),
    ];

    println!("\nCreating posts for various Rust resources:");
    for (url, title, description) in &links {
        println!("\n  • {}", title);
        println!("    URL: {}", url);
        println!("    Desc: {}", description);

        // Actual call (commented out for demo):
        // let uri = agent.post_with_link_embed(
        //     &format!("Useful Rust resource: {}", title),
        //     url,
        //     title,
        //     description,
        //     None,
        // ).await?;
        // println!("    Posted: {}", uri);
    }

    println!("\n✓ Multiple posts with different link cards would be created");

    // ============================================================================
    // Example 6: Link Card with Custom Thumbnail
    // ============================================================================
    println!("\n\nExample 6: Link Card with Custom Thumbnail");
    println!("───────────────────────────────────────────");

    println!("\nWith custom thumbnail workflow:");
    println!("  1. Upload thumbnail image first");
    println!("  2. Get blob reference");
    println!("  3. Create post with link embed and blob reference");

    println!("\nAlternative (using post_with_link_card):");
    println!("  1. Read thumbnail image from file");
    println!("  2. Call post_with_link_card() - handles upload automatically");

    // Method 1: Manual thumbnail upload
    println!("\nMethod 1 (manual):");
    // let thumb_blob = agent.upload_blob(thumbnail_data, "image/jpeg").await?;
    // let uri = agent.post_with_link_embed(
    //     "Article with custom thumbnail",
    //     "https://example.com/article",
    //     "Article Title",
    //     "Article description",
    //     Some(thumb_blob),  // Pre-uploaded blob
    // ).await?;

    // Method 2: Automatic thumbnail upload
    println!("\nMethod 2 (automatic - recommended):");
    // let thumbnail_data = std::fs::read("thumbnail.jpg")?;
    // let uri = agent.post_with_link_card(
    //     "Article with auto-uploaded thumbnail",
    //     "https://example.com/article",
    //     "Article Title",
    //     "Article description",
    //     thumbnail_data,  // Uploads automatically
    // ).await?;

    println!("\n✓ Both methods would work for adding thumbnails");

    // ============================================================================
    // Example 7: Complete Working Flow
    // ============================================================================
    println!("\n\nExample 7: Complete Working Flow");
    println!("─────────────────────────────────");

    println!("\nTo use this in a real application:\n");

    println!("1. Login:");
    println!("   let agent = Agent::new(\"https://bsky.social\".to_string());");
    println!("   agent.login(\"your-handle.bsky.social\", \"your-app-password\").await?;\n");

    println!("2. Create post with link card (no thumbnail):");
    println!("   let uri = agent.post_with_link_embed(");
    println!("       \"Check out this article!\",");
    println!("       \"https://example.com\",");
    println!("       \"Article Title\",");
    println!("       \"Article description\",");
    println!("       None,  // No thumbnail");
    println!("   ).await?;\n");

    println!("3. Create post with link card and thumbnail:");
    println!("   let thumb = std::fs::read(\"thumbnail.jpg\")?;");
    println!("   let uri = agent.post_with_link_card(");
    println!("       \"Great article!\",");
    println!("       \"https://example.com\",");
    println!("       \"Article Title\",");
    println!("       \"Article description\",");
    println!("       thumb,  // Uploads automatically");
    println!("   ).await?;\n");

    println!("4. Reply with link card:");
    println!("   let uri = agent.reply_with_link_embed(");
    println!("       \"Here's a related link!\",");
    println!("       \"https://related.com\",");
    println!("       \"Related Article\",");
    println!("       \"Related description\",");
    println!("       None,  // No thumbnail");
    println!("       &parent_uri,");
    println!("       &parent_cid,");
    println!("       &root_uri,");
    println!("       &root_cid,");
    println!("   ).await?;");

    println!("\n✓ Link embed demo complete!");

    println!("\nThe Rust ATProto SDK now supports:");
    println!("  ✓ Link preview cards (external embeds)");
    println!("  ✓ Custom thumbnails for link cards");
    println!("  ✓ Automatic thumbnail upload");
    println!("  ✓ Link cards in replies");
    println!("  ✓ Automatic facet detection with link cards");
    println!("  ✓ Rich text (mentions, links, hashtags) with link cards");

    Ok(())
}
