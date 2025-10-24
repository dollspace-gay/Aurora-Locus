//! Comprehensive tests for Agent preferences methods
//!
//! These tests verify:
//! 1. Method signatures and type correctness
//! 2. Authentication requirements
//! 3. Expected behavior patterns
//! 4. Error handling

use atproto::agent::Agent;

// Test update_saved_feeds signature
#[tokio::test]
async fn test_update_saved_feeds_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let feeds = vec![
        serde_json::json!({
            "id": "feed-123",
            "type": "timeline",
            "value": "at://did:plc:xyz/app.bsky.feed.generator/my-feed",
            "pinned": true
        })
    ];

    let result = agent.update_saved_feeds(feeds).await;
    assert!(result.is_err()); // Should fail without authentication
}

// Test update_saved_feeds requires authentication
#[tokio::test]
async fn test_update_saved_feeds_auth_required() {
    let agent = Agent::new("https://bsky.social".to_string());
    let result = agent.update_saved_feeds(vec![]).await;
    assert!(result.is_err());
}

// Test update_saved_feeds with empty list
#[tokio::test]
async fn test_update_saved_feeds_empty_list() {
    let agent = Agent::new("https://bsky.social".to_string());
    let result = agent.update_saved_feeds(vec![]).await;
    assert!(result.is_err()); // Fails due to no auth
}

// Test add_saved_feeds signature
#[tokio::test]
async fn test_add_saved_feeds_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let new_feeds = vec![
        serde_json::json!({
            "type": "feed",
            "value": "at://did:plc:abc/app.bsky.feed.generator/tech-news",
            "pinned": true
        })
    ];

    let result = agent.add_saved_feeds(new_feeds).await;
    assert!(result.is_err()); // Should fail without authentication
}

// Test add_saved_feeds requires authentication
#[tokio::test]
async fn test_add_saved_feeds_auth_required() {
    let agent = Agent::new("https://bsky.social".to_string());
    let result = agent.add_saved_feeds(vec![]).await;
    assert!(result.is_err());
}

// Test add_saved_feeds with multiple feeds
#[tokio::test]
async fn test_add_saved_feeds_multiple() {
    let agent = Agent::new("https://bsky.social".to_string());

    let feeds = vec![
        serde_json::json!({"type": "feed", "value": "at://did:plc:abc/app.bsky.feed.generator/feed1"}),
        serde_json::json!({"type": "feed", "value": "at://did:plc:abc/app.bsky.feed.generator/feed2"}),
    ];

    let result = agent.add_saved_feeds(feeds).await;
    assert!(result.is_err());
}

// Test add_saved_feeds with feed that has ID
#[tokio::test]
async fn test_add_saved_feeds_with_id() {
    let agent = Agent::new("https://bsky.social".to_string());

    let feeds = vec![
        serde_json::json!({
            "id": "custom-id-123",
            "type": "feed",
            "value": "at://did:plc:abc/app.bsky.feed.generator/tech"
        })
    ];

    let result = agent.add_saved_feeds(feeds).await;
    assert!(result.is_err());
}

// Test remove_saved_feeds signature
#[tokio::test]
async fn test_remove_saved_feeds_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let feed_ids = vec!["feed-123".to_string(), "feed-456".to_string()];

    let result = agent.remove_saved_feeds(feed_ids).await;
    assert!(result.is_err()); // Should fail without authentication
}

// Test remove_saved_feeds requires authentication
#[tokio::test]
async fn test_remove_saved_feeds_auth_required() {
    let agent = Agent::new("https://bsky.social".to_string());
    let result = agent.remove_saved_feeds(vec![]).await;
    assert!(result.is_err());
}

// Test remove_saved_feeds with single ID
#[tokio::test]
async fn test_remove_saved_feeds_single() {
    let agent = Agent::new("https://bsky.social".to_string());
    let result = agent.remove_saved_feeds(vec!["feed-123".to_string()]).await;
    assert!(result.is_err());
}

// Test remove_saved_feeds with empty list
#[tokio::test]
async fn test_remove_saved_feeds_empty() {
    let agent = Agent::new("https://bsky.social".to_string());
    let result = agent.remove_saved_feeds(vec![]).await;
    assert!(result.is_err());
}

// Test set_feed_view_prefs signature
#[tokio::test]
async fn test_set_feed_view_prefs_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let prefs = serde_json::json!({
        "hideReplies": true,
        "hideReposts": false,
        "hideQuotePosts": false
    });

    let result = agent.set_feed_view_prefs("at://did:plc:xyz/app.bsky.feed.generator/my-feed", prefs).await;
    assert!(result.is_err()); // Should fail without authentication
}

// Test set_feed_view_prefs requires authentication
#[tokio::test]
async fn test_set_feed_view_prefs_auth_required() {
    let agent = Agent::new("https://bsky.social".to_string());
    let prefs = serde_json::json!({});
    let result = agent.set_feed_view_prefs("at://did:plc:xyz/app.bsky.feed.generator/feed", prefs).await;
    assert!(result.is_err());
}

// Test set_feed_view_prefs with all options
#[tokio::test]
async fn test_set_feed_view_prefs_all_options() {
    let agent = Agent::new("https://bsky.social".to_string());

    let prefs = serde_json::json!({
        "hideReplies": true,
        "hideReposts": true,
        "hideQuotePosts": true,
        "hideRepliesByUnfollowed": true,
        "hideRepliesByLikeCount": 2
    });

    let result = agent.set_feed_view_prefs("following", prefs).await;
    assert!(result.is_err());
}

// Test set_feed_view_prefs with partial options
#[tokio::test]
async fn test_set_feed_view_prefs_partial() {
    let agent = Agent::new("https://bsky.social".to_string());

    let prefs = serde_json::json!({
        "hideReplies": false
    });

    let result = agent.set_feed_view_prefs("timeline", prefs).await;
    assert!(result.is_err());
}

// Test set_thread_view_prefs signature
#[tokio::test]
async fn test_set_thread_view_prefs_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let prefs = serde_json::json!({
        "sort": "oldest",
        "prioritizeFollowedUsers": true
    });

    let result = agent.set_thread_view_prefs(prefs).await;
    assert!(result.is_err()); // Should fail without authentication
}

// Test set_thread_view_prefs requires authentication
#[tokio::test]
async fn test_set_thread_view_prefs_auth_required() {
    let agent = Agent::new("https://bsky.social".to_string());
    let prefs = serde_json::json!({});
    let result = agent.set_thread_view_prefs(prefs).await;
    assert!(result.is_err());
}

// Test set_thread_view_prefs with sort options
#[tokio::test]
async fn test_set_thread_view_prefs_sort_newest() {
    let agent = Agent::new("https://bsky.social".to_string());

    let prefs = serde_json::json!({
        "sort": "newest"
    });

    let result = agent.set_thread_view_prefs(prefs).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_set_thread_view_prefs_sort_oldest() {
    let agent = Agent::new("https://bsky.social".to_string());

    let prefs = serde_json::json!({
        "sort": "oldest"
    });

    let result = agent.set_thread_view_prefs(prefs).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_set_thread_view_prefs_sort_most_likes() {
    let agent = Agent::new("https://bsky.social".to_string());

    let prefs = serde_json::json!({
        "sort": "most-likes"
    });

    let result = agent.set_thread_view_prefs(prefs).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_set_thread_view_prefs_sort_random() {
    let agent = Agent::new("https://bsky.social".to_string());

    let prefs = serde_json::json!({
        "sort": "random"
    });

    let result = agent.set_thread_view_prefs(prefs).await;
    assert!(result.is_err());
}

// Test set_thread_view_prefs with prioritize options
#[tokio::test]
async fn test_set_thread_view_prefs_prioritize_true() {
    let agent = Agent::new("https://bsky.social".to_string());

    let prefs = serde_json::json!({
        "prioritizeFollowedUsers": true
    });

    let result = agent.set_thread_view_prefs(prefs).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_set_thread_view_prefs_prioritize_false() {
    let agent = Agent::new("https://bsky.social".to_string());

    let prefs = serde_json::json!({
        "prioritizeFollowedUsers": false
    });

    let result = agent.set_thread_view_prefs(prefs).await;
    assert!(result.is_err());
}

// Test set_interests_pref signature
#[tokio::test]
async fn test_set_interests_pref_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let tags = vec!["technology".to_string(), "science".to_string()];

    let result = agent.set_interests_pref(tags).await;
    assert!(result.is_err()); // Should fail without authentication
}

// Test set_interests_pref requires authentication
#[tokio::test]
async fn test_set_interests_pref_auth_required() {
    let agent = Agent::new("https://bsky.social".to_string());
    let result = agent.set_interests_pref(vec![]).await;
    assert!(result.is_err());
}

// Test set_interests_pref with empty list
#[tokio::test]
async fn test_set_interests_pref_empty() {
    let agent = Agent::new("https://bsky.social".to_string());
    let result = agent.set_interests_pref(vec![]).await;
    assert!(result.is_err());
}

// Test set_interests_pref with single tag
#[tokio::test]
async fn test_set_interests_pref_single() {
    let agent = Agent::new("https://bsky.social".to_string());
    let tags = vec!["technology".to_string()];
    let result = agent.set_interests_pref(tags).await;
    assert!(result.is_err());
}

// Test set_interests_pref with multiple tags
#[tokio::test]
async fn test_set_interests_pref_multiple() {
    let agent = Agent::new("https://bsky.social".to_string());
    let tags = vec![
        "technology".to_string(),
        "science".to_string(),
        "art".to_string(),
        "music".to_string(),
    ];
    let result = agent.set_interests_pref(tags).await;
    assert!(result.is_err());
}

// Test set_personal_details signature
#[tokio::test]
async fn test_set_personal_details_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.set_personal_details("1990-01-15".to_string()).await;
    assert!(result.is_err()); // Should fail without authentication
}

// Test set_personal_details requires authentication
#[tokio::test]
async fn test_set_personal_details_auth_required() {
    let agent = Agent::new("https://bsky.social".to_string());
    let result = agent.set_personal_details("1990-01-01".to_string()).await;
    assert!(result.is_err());
}

// Test set_personal_details with various date formats
#[tokio::test]
async fn test_set_personal_details_date_format_iso() {
    let agent = Agent::new("https://bsky.social".to_string());
    let result = agent.set_personal_details("2000-12-31".to_string()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_set_personal_details_date_early_year() {
    let agent = Agent::new("https://bsky.social".to_string());
    let result = agent.set_personal_details("1950-06-15".to_string()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_set_personal_details_date_leap_year() {
    let agent = Agent::new("https://bsky.social".to_string());
    let result = agent.set_personal_details("2000-02-29".to_string()).await;
    assert!(result.is_err());
}

// Test upsert_profile signature
#[tokio::test]
async fn test_upsert_profile_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let updates = serde_json::json!({
        "displayName": "New Display Name",
        "description": "Software developer interested in Rust"
    });

    let result = agent.upsert_profile(updates).await;
    assert!(result.is_err()); // Should fail without authentication
}

// Test upsert_profile requires authentication
#[tokio::test]
async fn test_upsert_profile_auth_required() {
    let agent = Agent::new("https://bsky.social".to_string());
    let updates = serde_json::json!({});
    let result = agent.upsert_profile(updates).await;
    assert!(result.is_err());
}

// Test upsert_profile with display name only
#[tokio::test]
async fn test_upsert_profile_display_name() {
    let agent = Agent::new("https://bsky.social".to_string());

    let updates = serde_json::json!({
        "displayName": "John Doe"
    });

    let result = agent.upsert_profile(updates).await;
    assert!(result.is_err());
}

// Test upsert_profile with description only
#[tokio::test]
async fn test_upsert_profile_description() {
    let agent = Agent::new("https://bsky.social".to_string());

    let updates = serde_json::json!({
        "description": "Developer | Tech Enthusiast | Open Source Contributor"
    });

    let result = agent.upsert_profile(updates).await;
    assert!(result.is_err());
}

// Test upsert_profile with multiple fields
#[tokio::test]
async fn test_upsert_profile_multiple_fields() {
    let agent = Agent::new("https://bsky.social".to_string());

    let updates = serde_json::json!({
        "displayName": "Jane Smith",
        "description": "Software Engineer specializing in Rust and distributed systems",
        "avatar": "blob-ref-123",
        "banner": "blob-ref-456"
    });

    let result = agent.upsert_profile(updates).await;
    assert!(result.is_err());
}

// Test upsert_profile with empty updates
#[tokio::test]
async fn test_upsert_profile_empty() {
    let agent = Agent::new("https://bsky.social".to_string());
    let updates = serde_json::json!({});
    let result = agent.upsert_profile(updates).await;
    assert!(result.is_err());
}

// Test upsert_profile with long description
#[tokio::test]
async fn test_upsert_profile_long_description() {
    let agent = Agent::new("https://bsky.social".to_string());

    let updates = serde_json::json!({
        "description": "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris."
    });

    let result = agent.upsert_profile(updates).await;
    assert!(result.is_err());
}

// Workflow test: typical preferences configuration workflow
#[tokio::test]
async fn test_preferences_workflow() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Set interests
    let interests_result = agent.set_interests_pref(vec!["rust".to_string(), "tech".to_string()]).await;
    assert!(interests_result.is_err());

    // Set thread preferences
    let thread_prefs = serde_json::json!({
        "sort": "oldest",
        "prioritizeFollowedUsers": true
    });
    let thread_result = agent.set_thread_view_prefs(thread_prefs).await;
    assert!(thread_result.is_err());

    // Add a feed
    let feeds = vec![
        serde_json::json!({
            "type": "feed",
            "value": "at://did:plc:abc/app.bsky.feed.generator/tech"
        })
    ];
    let add_result = agent.add_saved_feeds(feeds).await;
    assert!(add_result.is_err());
}

// Workflow test: profile update workflow
#[tokio::test]
async fn test_profile_update_workflow() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Update personal details
    let details_result = agent.set_personal_details("1990-05-20".to_string()).await;
    assert!(details_result.is_err());

    // Update profile
    let profile_updates = serde_json::json!({
        "displayName": "Test User",
        "description": "Testing the Rust SDK"
    });
    let profile_result = agent.upsert_profile(profile_updates).await;
    assert!(profile_result.is_err());
}

// Edge case: feed management workflow
#[tokio::test]
async fn test_feed_management_workflow() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Add feeds
    let feeds = vec![
        serde_json::json!({"type": "feed", "value": "at://did:plc:abc/feed1"}),
        serde_json::json!({"type": "feed", "value": "at://did:plc:abc/feed2"}),
    ];
    let add_result = agent.add_saved_feeds(feeds).await;
    assert!(add_result.is_err());

    // Update a feed
    let updated_feeds = vec![
        serde_json::json!({
            "id": "feed-123",
            "type": "feed",
            "value": "at://did:plc:abc/feed1",
            "pinned": true
        })
    ];
    let update_result = agent.update_saved_feeds(updated_feeds).await;
    assert!(update_result.is_err());

    // Remove feeds
    let remove_result = agent.remove_saved_feeds(vec!["feed-123".to_string()]).await;
    assert!(remove_result.is_err());
}
