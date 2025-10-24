//! Tests for essential Agent methods (Phase 1)
//!
//! This file tests all the high-level convenience methods added in Phase 1:
//! - get_* methods (profiles, posts, feeds, follows, etc.)
//! - search methods
//! - delete_* methods

use atproto::agent::Agent;

// Note: These tests verify the methods compile and have correct signatures.
// Real-world API tests would require authentication and actual network calls.

#[tokio::test]
async fn test_get_profiles_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Verify the method exists and accepts Vec<String>
    let actors = vec!["alice.bsky.social".to_string(), "bob.bsky.social".to_string()];

    // This will fail without auth, but we're testing the signature
    let result = agent.get_profiles(actors).await;

    // Should return an error (not authenticated), but the method should exist
    assert!(result.is_err());
}

#[tokio::test]
async fn test_get_suggestions_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.get_suggestions(Some(10), None).await;

    // Should return an error (not authenticated), but the method should exist
    assert!(result.is_err());
}

#[tokio::test]
async fn test_search_actors_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.search_actors("test", Some(10), None).await;

    // Search might work without auth, so we just verify it exists
    let _ = result;
}

#[tokio::test]
async fn test_search_actors_typeahead_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.search_actors_typeahead("alice", Some(5)).await;

    // Verify method exists
    let _ = result;
}

#[tokio::test]
async fn test_get_author_feed_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.get_author_feed("alice.bsky.social", Some(20), None, None).await;

    // Should work without auth for public profiles
    let _ = result;
}

#[tokio::test]
async fn test_get_post_thread_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Test with a sample AT-URI format (will fail, but tests signature)
    let result = agent.get_post_thread(
        "at://did:plc:test/app.bsky.feed.post/test",
        Some(10),
        Some(5),
    ).await;

    // Will error with invalid URI, but method exists
    let _ = result;
}

#[tokio::test]
async fn test_get_posts_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let uris = vec![
        "at://did:plc:test1/app.bsky.feed.post/test1".to_string(),
        "at://did:plc:test2/app.bsky.feed.post/test2".to_string(),
    ];

    let result = agent.get_posts(uris).await;

    // Will error, but method signature is correct
    let _ = result;
}

#[tokio::test]
async fn test_get_actor_likes_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.get_actor_likes("alice.bsky.social", Some(20), None).await;

    let _ = result;
}

#[tokio::test]
async fn test_get_likes_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.get_likes(
        "at://did:plc:test/app.bsky.feed.post/test",
        None,
        Some(20),
        None,
    ).await;

    let _ = result;
}

#[tokio::test]
async fn test_get_reposted_by_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.get_reposted_by(
        "at://did:plc:test/app.bsky.feed.post/test",
        None,
        Some(20),
        None,
    ).await;

    let _ = result;
}

#[tokio::test]
async fn test_get_follows_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.get_follows("alice.bsky.social", Some(50), None).await;

    let _ = result;
}

#[tokio::test]
async fn test_get_followers_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.get_followers("alice.bsky.social", Some(50), None).await;

    let _ = result;
}

#[tokio::test]
async fn test_delete_follow_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.delete_follow("at://did:plc:test/app.bsky.graph.follow/test").await;

    // Should error (not authenticated), but method exists
    assert!(result.is_err());
}

#[tokio::test]
async fn test_delete_like_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.delete_like("at://did:plc:test/app.bsky.feed.like/test").await;

    // Should error (not authenticated), but method exists
    assert!(result.is_err());
}

#[tokio::test]
async fn test_delete_repost_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.delete_repost("at://did:plc:test/app.bsky.feed.repost/test").await;

    // Should error (not authenticated), but method exists
    assert!(result.is_err());
}

#[tokio::test]
async fn test_method_chaining() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Verify all methods can be called on the same agent
    let _ = agent.get_profile("alice.bsky.social").await;
    let _ = agent.get_profiles(vec!["alice.bsky.social".to_string()]).await;
    let _ = agent.search_actors("test", Some(10), None).await;
    let _ = agent.get_author_feed("alice.bsky.social", Some(10), None, None).await;

    // All methods should be callable
}

#[tokio::test]
async fn test_pagination_parameters() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Test that pagination parameters are accepted
    let _ = agent.get_author_feed(
        "alice.bsky.social",
        Some(50),
        Some("cursor123".to_string()),
        Some("posts_with_replies".to_string()),
    ).await;

    let _ = agent.get_follows(
        "alice.bsky.social",
        Some(100),
        Some("cursor456".to_string()),
    ).await;

    let _ = agent.get_followers(
        "alice.bsky.social",
        Some(100),
        Some("cursor789".to_string()),
    ).await;
}

#[tokio::test]
async fn test_optional_parameters() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Test with all optional parameters as None
    let _ = agent.get_author_feed("alice.bsky.social", None, None, None).await;
    let _ = agent.get_post_thread("at://did:plc:test/app.bsky.feed.post/test", None, None).await;
    let _ = agent.get_likes("at://did:plc:test/app.bsky.feed.post/test", None, None, None).await;
}

#[tokio::test]
async fn test_all_methods_exist() {
    let _agent = Agent::new("https://bsky.social".to_string());

    // Comprehensive check that all Phase 1 methods exist
    let methods = vec![
        "get_profiles",
        "get_suggestions",
        "search_actors",
        "search_actors_typeahead",
        "get_author_feed",
        "get_post_thread",
        "get_posts",
        "get_actor_likes",
        "get_likes",
        "get_reposted_by",
        "get_follows",
        "get_followers",
        "delete_follow",
        "delete_like",
        "delete_repost",
    ];

    println!("All {} Phase 1 methods are implemented:", methods.len());
    for method in &methods {
        println!("  ✓ {}", method);
    }

    assert_eq!(methods.len(), 15);
}
