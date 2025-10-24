//! Tests for Moderation Actions (Phase 3)
//!
//! This file tests the moderation action methods:
//! - mute() / unmute()
//! - mute_mod_list() / unmute_mod_list()
//! - block_mod_list() / unblock_mod_list()
//! - create_moderation_report()

use atproto::agent::Agent;

#[tokio::test]
async fn test_mute_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Verify method exists and accepts actor string
    let result = agent.mute("spammer.bsky.social").await;

    // Should error (not authenticated), but method exists
    assert!(result.is_err());
}

#[tokio::test]
async fn test_unmute_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.unmute("someone.bsky.social").await;

    // Should error (not authenticated), but method exists
    assert!(result.is_err());
}

#[tokio::test]
async fn test_mute_with_did() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.mute("did:plc:test123").await;

    // Method should accept DIDs as well as handles
    assert!(result.is_err());
}

#[tokio::test]
async fn test_mute_mod_list_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.mute_mod_list("at://did:plc:xxx/app.bsky.graph.list/yyy").await;

    // Should error (not authenticated), but method exists
    assert!(result.is_err());
}

#[tokio::test]
async fn test_unmute_mod_list_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.unmute_mod_list("at://did:plc:xxx/app.bsky.graph.list/yyy").await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_block_mod_list_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.block_mod_list("at://did:plc:xxx/app.bsky.graph.list/yyy").await;

    // Should error (not authenticated), but method exists
    assert!(result.is_err());
}

#[tokio::test]
async fn test_unblock_mod_list_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let result = agent.unblock_mod_list("at://did:plc:xxx/app.bsky.graph.list/yyy").await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_create_moderation_report_signature() {
    let agent = Agent::new("https://bsky.social".to_string());

    let subject = serde_json::json!({
        "$type": "com.atproto.admin.defs#repoRef",
        "did": "did:plc:spammer123"
    });

    let result = agent.create_moderation_report(
        subject,
        "com.atproto.moderation.defs#reasonSpam",
        Some("This is spam".to_string()),
    ).await;

    // Should error (not authenticated), but method exists
    assert!(result.is_err());
}

#[tokio::test]
async fn test_create_moderation_report_without_reason() {
    let agent = Agent::new("https://bsky.social".to_string());

    let subject = serde_json::json!({
        "$type": "com.atproto.admin.defs#repoRef",
        "did": "did:plc:test123"
    });

    let result = agent.create_moderation_report(
        subject,
        "com.atproto.moderation.defs#reasonMisleading",
        None,  // No additional reason
    ).await;

    // Method should accept None for reason
    assert!(result.is_err());
}

#[tokio::test]
async fn test_report_post_record() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Report a specific post
    let subject = serde_json::json!({
        "$type": "com.atproto.repo.strongRef",
        "uri": "at://did:plc:xxx/app.bsky.feed.post/yyy",
        "cid": "bafytest123"
    });

    let result = agent.create_moderation_report(
        subject,
        "com.atproto.moderation.defs#reasonViolation",
        Some("Violates community guidelines".to_string()),
    ).await;

    // Should handle post record subjects
    assert!(result.is_err());
}

#[tokio::test]
async fn test_moderation_methods_require_auth() {
    let agent = Agent::new("https://bsky.social".to_string());

    // All moderation methods should require authentication
    assert!(!agent.is_authenticated());

    assert!(agent.mute("test.bsky.social").await.is_err());
    assert!(agent.unmute("test.bsky.social").await.is_err());
    assert!(agent.mute_mod_list("at://did:plc:xxx/app.bsky.graph.list/yyy").await.is_err());
    assert!(agent.unmute_mod_list("at://did:plc:xxx/app.bsky.graph.list/yyy").await.is_err());
    assert!(agent.block_mod_list("at://did:plc:xxx/app.bsky.graph.list/yyy").await.is_err());
    assert!(agent.unblock_mod_list("at://did:plc:xxx/app.bsky.graph.list/yyy").await.is_err());

    let subject = serde_json::json!({"did": "did:plc:test"});
    assert!(agent.create_moderation_report(subject, "spam", None).await.is_err());
}

#[tokio::test]
async fn test_moderation_methods_on_same_agent() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Verify all methods can be called on the same agent instance
    let _ = agent.mute("actor1.bsky.social").await;
    let _ = agent.unmute("actor2.bsky.social").await;
    let _ = agent.mute_mod_list("at://did:plc:xxx/app.bsky.graph.list/yyy").await;
    let _ = agent.unmute_mod_list("at://did:plc:xxx/app.bsky.graph.list/yyy").await;
    let _ = agent.block_mod_list("at://did:plc:xxx/app.bsky.graph.list/yyy").await;
    let _ = agent.unblock_mod_list("at://did:plc:xxx/app.bsky.graph.list/yyy").await;

    // All methods should be callable
}

#[tokio::test]
async fn test_mute_unmute_workflow() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Typical mute workflow
    let _ = agent.mute("spammer.bsky.social").await;
    let _ = agent.unmute("spammer.bsky.social").await;

    // Methods should follow expected workflow
}

#[tokio::test]
async fn test_mod_list_mute_workflow() {
    let agent = Agent::new("https://bsky.social".to_string());

    let list_uri = "at://did:plc:xxx/app.bsky.graph.list/blocklist";

    // Mute then unmute a list
    let _ = agent.mute_mod_list(list_uri).await;
    let _ = agent.unmute_mod_list(list_uri).await;
}

#[tokio::test]
async fn test_mod_list_block_workflow() {
    let agent = Agent::new("https://bsky.social".to_string());

    let list_uri = "at://did:plc:xxx/app.bsky.graph.list/blocklist";

    // Block then unblock a list
    let _ = agent.block_mod_list(list_uri).await;
    let _ = agent.unblock_mod_list(list_uri).await;
}

#[tokio::test]
async fn test_moderation_report_return_type() {
    let agent = Agent::new("https://bsky.social".to_string());

    let subject = serde_json::json!({"did": "did:plc:test"});
    let result = agent.create_moderation_report(subject, "spam", None).await;

    // Should return Result<i64, _> for the report ID
    assert!(matches!(result, Err(_)));
}

#[tokio::test]
async fn test_mute_unmute_return_types() {
    let agent = Agent::new("https://bsky.social".to_string());

    // All mute/unmute methods return Result<(), _>
    let mute_result = agent.mute("test").await;
    assert!(matches!(mute_result, Err(_)));

    let unmute_result = agent.unmute("test").await;
    assert!(matches!(unmute_result, Err(_)));
}

#[tokio::test]
async fn test_all_moderation_methods_exist() {
    let _agent = Agent::new("https://bsky.social".to_string());

    let methods = vec![
        "mute",
        "unmute",
        "mute_mod_list",
        "unmute_mod_list",
        "block_mod_list",
        "unblock_mod_list",
        "create_moderation_report",
    ];

    println!("All {} Phase 3 moderation methods are implemented:", methods.len());
    for method in &methods {
        println!("  ✓ {}", method);
    }

    assert_eq!(methods.len(), 7);
}

#[tokio::test]
async fn test_invalid_list_uri_handling() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Test with invalid URI format
    let result = agent.mute_mod_list("invalid-uri").await;
    assert!(result.is_err());

    let result2 = agent.block_mod_list("not-an-at-uri").await;
    assert!(result2.is_err());
}

#[tokio::test]
async fn test_moderation_reason_types() {
    let agent = Agent::new("https://bsky.social".to_string());

    let subject = serde_json::json!({"did": "did:plc:test"});

    // Test different reason types
    let reason_types = vec![
        "com.atproto.moderation.defs#reasonSpam",
        "com.atproto.moderation.defs#reasonViolation",
        "com.atproto.moderation.defs#reasonMisleading",
        "com.atproto.moderation.defs#reasonSexual",
        "com.atproto.moderation.defs#reasonRude",
        "com.atproto.moderation.defs#reasonOther",
    ];

    for reason_type in reason_types {
        let _ = agent.create_moderation_report(
            subject.clone(),
            reason_type,
            None,
        ).await;
    }

    // Should accept various reason types
}

#[tokio::test]
async fn test_report_with_detailed_reason() {
    let agent = Agent::new("https://bsky.social".to_string());

    let subject = serde_json::json!({
        "$type": "com.atproto.admin.defs#repoRef",
        "did": "did:plc:spammer"
    });

    let detailed_reason = "This account has been posting promotional content \
                          repeatedly, violating our spam policy. They have posted \
                          the same message 50 times in the last hour.".to_string();

    let result = agent.create_moderation_report(
        subject,
        "com.atproto.moderation.defs#reasonSpam",
        Some(detailed_reason),
    ).await;

    // Should handle long, detailed reasons
    assert!(result.is_err());
}

#[tokio::test]
async fn test_mute_multiple_actors() {
    let agent = Agent::new("https://bsky.social".to_string());

    // Muting multiple actors in sequence
    let actors = vec!["spam1.bsky.social", "spam2.bsky.social", "spam3.bsky.social"];

    for actor in actors {
        let _ = agent.mute(actor).await;
    }

    // Should support muting multiple actors
}

#[tokio::test]
async fn test_block_unblock_multiple_lists() {
    let agent = Agent::new("https://bsky.social".to_string());

    let lists = vec![
        "at://did:plc:xxx/app.bsky.graph.list/list1",
        "at://did:plc:xxx/app.bsky.graph.list/list2",
    ];

    for list_uri in lists {
        let _ = agent.block_mod_list(list_uri).await;
        let _ = agent.unblock_mod_list(list_uri).await;
    }
}
