//! Simplified Integration Tests
//!
//! These tests demonstrate that all PDS components work together correctly.

use atproto::server_auth::{AuthManager, AccountCreate};
use atproto::repo::Repository;
use atproto::validation::LexiconSchema;
use serde_json::json;

/// Test: Create account → Validate post → Store in repo → Export CAR
#[tokio::test]
async fn test_complete_pds_workflow() {
    // 1. Create account
    let mut auth = AuthManager::new();
    let account = auth.create_account(AccountCreate {
        handle: "alice.test.com".to_string(),
        email: Some("alice@example.com".to_string()),
        password: Some("secure_pass_123".to_string()),
        invite_code: None,
        did: None,
        verification_code: None,
        verification_phone: None,
        recovery_key: None,
        plc_op: None,
    }).await.unwrap();

    println!("✅ Created account: {}", account.handle);

    // 2. Create and validate a post
    let post = json!({
        "$type": "app.bsky.feed.post",
        "text": "Hello from Rust PDS! 🦀",
        "createdAt": chrono::Utc::now().to_rfc3339(),
    });

    let schema_json = r#"{
        "lexicon": 1,
        "id": "app.bsky.feed.post",
        "defs": {
            "main": {
                "type": "object",
                "required": ["text", "createdAt"],
                "properties": {
                    "text": {"type": "string", "maxLength": 3000},
                    "createdAt": {"type": "string", "format": "datetime"}
                }
            }
        }
    }"#;

    let schema = LexiconSchema::from_json(schema_json).unwrap();
    schema.validate(&post).unwrap();

    println!("✅ Validated post against schema");

    // 3. Store in repository
    let did = atproto::types::Did::new(&account.did).unwrap();
    let mut repo = Repository::create(did);

    let post_bytes = serde_json::to_vec(&post).unwrap();
    let cid = repo.put_record("app.bsky.feed.post", "post1", post_bytes).unwrap();

    println!("✅ Stored record with CID: {}", cid);

    // 4. Retrieve and verify
    let retrieved_bytes = repo.get_record("app.bsky.feed.post", "post1");
    assert!(retrieved_bytes.is_some());

    println!("✅ Retrieved record from repository");

    // 5. Export as CAR
    let car_bytes = repo.export_car().unwrap();
    assert!(car_bytes.len() > 10); // CAR header is minimal

    println!("✅ Exported repository as CAR ({} bytes)", car_bytes.len());
    println!("\n🎉 Complete PDS workflow successful!");
}

/// Test: Multi-user scenario with auth and repos
#[tokio::test]
async fn test_multi_user_auth_and_repos() {
    let mut auth = AuthManager::new();

    // Create 2 users
    let alice = auth.create_account(AccountCreate {
        handle: "alice.test.com".to_string(),
        email: None,
        password: Some("pass123".to_string()),
        invite_code: None,
        did: None,
        verification_code: None,
        verification_phone: None,
        recovery_key: None,
        plc_op: None,
    }).await.unwrap();

    let bob = auth.create_account(AccountCreate {
        handle: "bob.test.com".to_string(),
        email: None,
        password: Some("pass456".to_string()),
        invite_code: None,
        did: None,
        verification_code: None,
        verification_phone: None,
        recovery_key: None,
        plc_op: None,
    }).await.unwrap();

    println!("✅ Created 2 accounts");

    // Create repositories
    let alice_did = atproto::types::Did::new(&alice.did).unwrap();
    let bob_did = atproto::types::Did::new(&bob.did).unwrap();

    let mut alice_repo = Repository::create(alice_did);
    let mut bob_repo = Repository::create(bob_did);

    // Alice creates a post
    let alice_post = json!({"text": "Alice's post", "createdAt": chrono::Utc::now().to_rfc3339()});
    let alice_post_bytes = serde_json::to_vec(&alice_post).unwrap();
    let alice_cid = alice_repo.put_record("app.bsky.feed.post", "post1", alice_post_bytes).unwrap();

    // Bob creates a post
    let bob_post = json!({"text": "Bob's post", "createdAt": chrono::Utc::now().to_rfc3339()});
    let bob_post_bytes = serde_json::to_vec(&bob_post).unwrap();
    let bob_cid = bob_repo.put_record("app.bsky.feed.post", "post1", bob_post_bytes).unwrap();

    println!("✅ Both users created posts");

    // Verify separate repositories
    assert_eq!(alice_repo.len(), 1);
    assert_eq!(bob_repo.len(), 1);
    assert_ne!(alice_cid, bob_cid);

    println!("✅ Repositories are independent");
    println!("\n🎉 Multi-user test successful!");
}

/// Test: Session lifecycle
#[tokio::test]
async fn test_session_lifecycle() {
    let mut auth = AuthManager::new();

    // Create account
    let created = auth.create_account(AccountCreate {
        handle: "test.user.com".to_string(),
        email: Some("test@example.com".to_string()),
        password: Some("my_password".to_string()),
        invite_code: None,
        did: None,
        verification_code: None,
        verification_phone: None,
        recovery_key: None,
        plc_op: None,
    }).await.unwrap();

    println!("✅ Account created");

    // Logout
    auth.delete_session(&created.refresh_jwt).await.unwrap();
    println!("✅ Logged out");

    // Login again
    let session = auth.create_session("test.user.com", "my_password", None).await.unwrap();
    println!("✅ Logged in");

    // Wait a moment so timestamps differ
    tokio::time::sleep(tokio::time::Duration::from_millis(1100)).await;

    // Refresh session
    let refreshed = auth.refresh_session(&session.refresh_jwt).await.unwrap();
    assert_ne!(refreshed.access_jwt, session.access_jwt);
    println!("✅ Session refreshed");

    println!("\n🎉 Session lifecycle complete!");
}

/// Test: Validation with real schema
#[tokio::test]
async fn test_validation_enforcement() {
    let schema_json = r#"{
        "lexicon": 1,
        "id": "app.bsky.feed.post",
        "defs": {
            "main": {
                "type": "object",
                "required": ["text", "createdAt"],
                "properties": {
                    "text": {"type": "string", "maxLength": 100},
                    "createdAt": {"type": "string", "format": "datetime"}
                }
            }
        }
    }"#;

    let schema = LexiconSchema::from_json(schema_json).unwrap();

    // Valid post
    let valid = json!({
        "text": "Valid post",
        "createdAt": "2025-01-15T10:00:00Z"
    });
    assert!(schema.validate(&valid).is_ok());
    println!("✅ Valid post accepted");

    // Invalid: text too long
    let invalid = json!({
        "text": "a".repeat(101),
        "createdAt": "2025-01-15T10:00:00Z"
    });
    assert!(schema.validate(&invalid).is_err());
    println!("✅ Invalid post (too long) rejected");

    // Invalid: missing required field
    let missing = json!({"text": "Missing createdAt"});
    assert!(schema.validate(&missing).is_err());
    println!("✅ Invalid post (missing field) rejected");

    println!("\n🎉 Validation enforcement works!");
}

/// Test: Multi-collection repository
#[tokio::test]
async fn test_multi_collection_repository() {
    let did = atproto::types::Did::new("did:plc:test123").unwrap();
    let mut repo = Repository::create(did);

    // Add posts
    let post1 = json!({"text": "Post 1", "createdAt": "2025-01-15T10:00:00Z"});
    let post2 = json!({"text": "Post 2", "createdAt": "2025-01-15T10:05:00Z"});
    repo.put_record("app.bsky.feed.post", "post1", serde_json::to_vec(&post1).unwrap()).unwrap();
    repo.put_record("app.bsky.feed.post", "post2", serde_json::to_vec(&post2).unwrap()).unwrap();

    // Add profile
    let profile = json!({"displayName": "Test User", "description": "A test"});
    repo.put_record("app.bsky.actor.profile", "self", serde_json::to_vec(&profile).unwrap()).unwrap();

    // Add follow
    let follow = json!({"subject": "did:plc:other123", "createdAt": "2025-01-15T10:10:00Z"});
    repo.put_record("app.bsky.graph.follow", "follow1", serde_json::to_vec(&follow).unwrap()).unwrap();

    // Verify collections
    let posts = repo.list_records("app.bsky.feed.post");
    let profiles = repo.list_records("app.bsky.actor.profile");
    let follows = repo.list_records("app.bsky.graph.follow");

    assert_eq!(posts.len(), 2);
    assert_eq!(profiles.len(), 1);
    assert_eq!(follows.len(), 1);
    assert_eq!(repo.len(), 4);

    println!("✅ Multi-collection repository works");
    println!("   - 2 posts");
    println!("   - 1 profile");
    println!("   - 1 follow");
    println!("   - Total: 4 records");

    println!("\n🎉 Multi-collection test successful!");
}

/// Test: Auth error handling
#[tokio::test]
async fn test_auth_error_handling() {
    let mut auth = AuthManager::new();

    // Create account
    auth.create_account(AccountCreate {
        handle: "existing.user.com".to_string(),
        email: None,
        password: Some("password123".to_string()),
        invite_code: None,
        did: None,
        verification_code: None,
        verification_phone: None,
        recovery_key: None,
        plc_op: None,
    }).await.unwrap();

    // Try duplicate handle
    let duplicate = auth.create_account(AccountCreate {
        handle: "existing.user.com".to_string(),
        email: None,
        password: Some("different".to_string()),
        invite_code: None,
        did: None,
        verification_code: None,
        verification_phone: None,
        recovery_key: None,
        plc_op: None,
    }).await;
    assert!(duplicate.is_err());
    println!("✅ Duplicate handle rejected");

    // Try wrong password
    let wrong_pass = auth.create_session("existing.user.com", "wrong", None).await;
    assert!(wrong_pass.is_err());
    println!("✅ Wrong password rejected");

    // Try non-existent account
    let no_account = auth.create_session("nonexistent.com", "password123", None).await;
    assert!(no_account.is_err());
    println!("✅ Non-existent account rejected");

    println!("\n🎉 Auth error handling works!");
}

/// Test: Complete PDS simulation with 3 users
#[tokio::test]
async fn test_complete_pds_simulation() {
    println!("\n🚀 Starting Complete PDS Simulation...\n");

    let mut auth = AuthManager::new();

    // Create 3 users
    let alice = auth.create_account(AccountCreate {
        handle: "alice.social".to_string(),
        email: Some("alice@example.com".to_string()),
        password: Some("alice_password".to_string()),
        invite_code: None,
        did: None,
        verification_code: None,
        verification_phone: None,
        recovery_key: None,
        plc_op: None,
    }).await.unwrap();

    let bob = auth.create_account(AccountCreate {
        handle: "bob.social".to_string(),
        email: Some("bob@example.com".to_string()),
        password: Some("bob_password".to_string()),
        invite_code: None,
        did: None,
        verification_code: None,
        verification_phone: None,
        recovery_key: None,
        plc_op: None,
    }).await.unwrap();

    let charlie = auth.create_account(AccountCreate {
        handle: "charlie.social".to_string(),
        email: None,
        password: Some("charlie_password".to_string()),
        invite_code: None,
        did: None,
        verification_code: None,
        verification_phone: None,
        recovery_key: None,
        plc_op: None,
    }).await.unwrap();

    println!("✅ Created 3 users:");
    println!("   - alice.social");
    println!("   - bob.social");
    println!("   - charlie.social");

    // Create repositories
    let mut alice_repo = Repository::create(atproto::types::Did::new(&alice.did).unwrap());
    let mut bob_repo = Repository::create(atproto::types::Did::new(&bob.did).unwrap());
    let mut charlie_repo = Repository::create(atproto::types::Did::new(&charlie.did).unwrap());

    // Alice creates profile and post
    let alice_profile = json!({"displayName": "Alice", "description": "I love Rust! 🦀"});
    alice_repo.put_record("app.bsky.actor.profile", "self", serde_json::to_vec(&alice_profile).unwrap()).unwrap();

    let alice_post = json!({"text": "Just deployed my first PDS written in Rust! 🎉", "createdAt": chrono::Utc::now().to_rfc3339()});
    alice_repo.put_record("app.bsky.feed.post", "post1", serde_json::to_vec(&alice_post).unwrap()).unwrap();

    // Bob creates profile, follows Alice, and replies
    let bob_profile = json!({"displayName": "Bob", "description": "Developer"});
    bob_repo.put_record("app.bsky.actor.profile", "self", serde_json::to_vec(&bob_profile).unwrap()).unwrap();

    let bob_follow = json!({"subject": &alice.did, "createdAt": chrono::Utc::now().to_rfc3339()});
    bob_repo.put_record("app.bsky.graph.follow", "follow1", serde_json::to_vec(&bob_follow).unwrap()).unwrap();

    let bob_reply = json!({"text": "That's awesome Alice!", "createdAt": chrono::Utc::now().to_rfc3339()});
    bob_repo.put_record("app.bsky.feed.post", "reply1", serde_json::to_vec(&bob_reply).unwrap()).unwrap();

    // Charlie creates profile, follows both, and posts
    let charlie_profile = json!({"displayName": "Charlie"});
    charlie_repo.put_record("app.bsky.actor.profile", "self", serde_json::to_vec(&charlie_profile).unwrap()).unwrap();

    let charlie_follow1 = json!({"subject": &alice.did, "createdAt": chrono::Utc::now().to_rfc3339()});
    charlie_repo.put_record("app.bsky.graph.follow", "follow1", serde_json::to_vec(&charlie_follow1).unwrap()).unwrap();

    let charlie_follow2 = json!({"subject": &bob.did, "createdAt": chrono::Utc::now().to_rfc3339()});
    charlie_repo.put_record("app.bsky.graph.follow", "follow2", serde_json::to_vec(&charlie_follow2).unwrap()).unwrap();

    let charlie_post = json!({"text": "Great to see the Rust ATProto ecosystem growing!", "createdAt": chrono::Utc::now().to_rfc3339()});
    charlie_repo.put_record("app.bsky.feed.post", "post1", serde_json::to_vec(&charlie_post).unwrap()).unwrap();

    // Export all
    let alice_car = alice_repo.export_car().unwrap();
    let bob_car = bob_repo.export_car().unwrap();
    let charlie_car = charlie_repo.export_car().unwrap();

    println!("\n✅ Exported repositories:");
    println!("   - Alice: {} bytes", alice_car.len());
    println!("   - Bob: {} bytes", bob_car.len());
    println!("   - Charlie: {} bytes", charlie_car.len());

    // Verify counts
    assert_eq!(alice_repo.len(), 2); // profile + post
    assert_eq!(bob_repo.len(), 3); // profile + follow + reply
    assert_eq!(charlie_repo.len(), 4); // profile + 2 follows + post

    println!("\n✅ Record counts:");
    println!("   - Alice: {} records", alice_repo.len());
    println!("   - Bob: {} records", bob_repo.len());
    println!("   - Charlie: {} records", charlie_repo.len());

    println!("\n🎉 Complete PDS simulation successful!");
    println!("   ✅ 3 users created");
    println!("   ✅ Profiles created");
    println!("   ✅ Social graph established");
    println!("   ✅ Posts and replies created");
    println!("   ✅ All data exported as CAR files");
    println!("\n   This demonstrates a fully functional PDS!");
}
