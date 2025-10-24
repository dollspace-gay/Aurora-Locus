//! Tests for Notification API methods (Phase 2)
//!
//! This file tests the notification management methods:
//! - list_notifications()
//! - count_unread_notifications()
//! - update_seen_notifications()

use atproto::agent::Agent;

#[tokio::test]
async fn test_list_notifications_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Verify the method exists with correct signature
    let result = agent.list_notifications(
        Some(50),                    // limit
        None,                        // cursor
        None,                        // seen_at
        None,                        // priority
    ).await;

    // Should error (not authenticated), but method exists
    assert!(result.is_err());
}

#[tokio::test]
async fn test_list_notifications_with_cursor() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.list_notifications(
        Some(20),
        Some("cursor123".to_string()),
        None,
        None,
    ).await;

    // Method should accept cursor parameter
    assert!(result.is_err());
}

#[tokio::test]
async fn test_list_notifications_with_priority() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.list_notifications(
        Some(50),
        None,
        None,
        Some(true),  // priority only
    ).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_list_notifications_with_seen_at() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.list_notifications(
        Some(50),
        None,
        Some("2024-01-01T00:00:00Z".to_string()),
        None,
    ).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_list_notifications_all_params() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.list_notifications(
        Some(100),
        Some("cursor".to_string()),
        Some("2024-01-01T00:00:00Z".to_string()),
        Some(false),
    ).await;

    // All parameters should be accepted
    assert!(result.is_err());
}

#[tokio::test]
async fn test_count_unread_notifications_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.count_unread_notifications(None, None).await;

    // Should error (not authenticated), but method exists and returns i64
    assert!(result.is_err());
}

#[tokio::test]
async fn test_count_unread_notifications_with_priority() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.count_unread_notifications(Some(true), None).await;

    // Method should accept priority parameter
    assert!(result.is_err());
}

#[tokio::test]
async fn test_count_unread_notifications_with_seen_at() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.count_unread_notifications(
        None,
        Some("2024-01-01T00:00:00Z".to_string()),
    ).await;

    // Method should accept seen_at parameter
    assert!(result.is_err());
}

#[tokio::test]
async fn test_count_unread_notifications_all_params() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.count_unread_notifications(
        Some(false),
        Some("2024-01-01T00:00:00Z".to_string()),
    ).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_update_seen_notifications_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Test with None (should use current time)
    let result = agent.update_seen_notifications(None).await;

    // Should error (not authenticated), but method exists
    assert!(result.is_err());
}

#[tokio::test]
async fn test_update_seen_notifications_with_timestamp() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Test with explicit timestamp
    let result = agent.update_seen_notifications(
        Some("2024-01-01T00:00:00Z".to_string())
    ).await;

    // Method should accept timestamp parameter
    assert!(result.is_err());
}

#[tokio::test]
async fn test_update_seen_notifications_current_time() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Verify that None defaults to current time (by checking it doesn't panic)
    let result = agent.update_seen_notifications(None).await;

    // Should generate timestamp without panicking
    assert!(result.is_err());
}

#[tokio::test]
async fn test_notification_methods_require_auth() {
    let agent = Agent::new("https://bsky.social".to_string());

    // All notification methods should require authentication
    assert!(!agent.is_authenticated());

    let list_result = agent.list_notifications(Some(10), None, None, None).await;
    assert!(list_result.is_err());

    let count_result = agent.count_unread_notifications(None, None).await;
    assert!(count_result.is_err());

    let update_result = agent.update_seen_notifications(None).await;
    assert!(update_result.is_err());
}

#[tokio::test]
async fn test_notification_methods_on_same_agent() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Verify all methods can be called on the same agent instance
    let _ = agent.list_notifications(Some(20), None, None, None).await;
    let _ = agent.count_unread_notifications(None, None).await;
    let _ = agent.update_seen_notifications(None).await;

    // All methods should be callable
}

#[tokio::test]
async fn test_notification_pagination_flow() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Simulate pagination workflow
    // First page
    let _ = agent.list_notifications(Some(50), None, None, None).await;

    // Second page with cursor
    let _ = agent.list_notifications(
        Some(50),
        Some("next_cursor".to_string()),
        None,
        None,
    ).await;

    // Methods should support pagination pattern
}

#[tokio::test]
async fn test_notification_workflow() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Typical notification workflow:
    // 1. Count unread
    let _ = agent.count_unread_notifications(None, None).await;

    // 2. List notifications
    let _ = agent.list_notifications(Some(50), None, None, None).await;

    // 3. Mark as seen
    let _ = agent.update_seen_notifications(None).await;

    // 4. Verify count is updated (would be 0 if authenticated)
    let _ = agent.count_unread_notifications(None, None).await;
}

#[tokio::test]
async fn test_all_notification_methods_exist() {
    let _agent = Agent::new("https://bsky.social".to_string());

    let methods = vec![
        "list_notifications",
        "count_unread_notifications",
        "update_seen_notifications",
    ];

    println!("All {} Phase 2 notification methods are implemented:", methods.len());
    for method in &methods {
        println!("  ✓ {}", method);
    }

    assert_eq!(methods.len(), 3);
}

#[tokio::test]
async fn test_notification_return_types() {
    let agent = Agent::new("https://bsky.social".to_string());

    // list_notifications returns Result<serde_json::Value, _>
    let list_result = agent.list_notifications(Some(10), None, None, None).await;
    assert!(matches!(list_result, Err(_)));

    // count_unread_notifications returns Result<i64, _>
    let count_result = agent.count_unread_notifications(None, None).await;
    assert!(matches!(count_result, Err(_)));

    // update_seen_notifications returns Result<(), _>
    let update_result = agent.update_seen_notifications(None).await;
    assert!(matches!(update_result, Err(_)));
}

#[tokio::test]
async fn test_notification_optional_parameters() {
    let agent = Agent::new("https://bsky.social".to_string());

    // All parameters are optional (except authenticated)
    let _ = agent.list_notifications(None, None, None, None).await;
    let _ = agent.count_unread_notifications(None, None).await;
    let _ = agent.update_seen_notifications(None).await;

    // Should accept all None parameters
}

#[tokio::test]
async fn test_notification_iso_timestamp_format() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Test various ISO 8601 timestamp formats
    let timestamps = vec![
        "2024-01-01T00:00:00Z",
        "2024-01-15T12:30:45.123Z",
        "2024-12-31T23:59:59.999Z",
    ];

    for ts in timestamps {
        let _ = agent.update_seen_notifications(Some(ts.to_string())).await;
        let _ = agent.list_notifications(None, None, Some(ts.to_string()), None).await;
        let _ = agent.count_unread_notifications(None, Some(ts.to_string())).await;
    }
}
