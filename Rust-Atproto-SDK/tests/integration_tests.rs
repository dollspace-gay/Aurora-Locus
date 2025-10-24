//! Integration tests for ATProto PDS components
//!
//! These tests demonstrate that all the PDS components work together:
//! - Server authentication
//! - Repository management
//! - Record validation
//! - MST storage
//! - CAR export
//!
//! This is a complete end-to-end PDS simulation without requiring
//! external network connections.

use atproto::server_auth::{AuthManager, AccountCreate};
use atproto::repo::Repository;
use atproto::validation::LexiconSchema;
use serde_json::json;

/// Helper to convert JSON to bytes for repository storage
fn json_to_bytes(value: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

/// Test complete PDS workflow: create account, create post, validate, store
#[tokio::test]
async fn test_complete_pds_workflow() {
    // Step 1: Create account with server auth
    let mut auth_manager = AuthManager::new();

    let alice_create = AccountCreate {
        handle: "alice.test.com".to_string(),
        email: Some("alice@example.com".to_string()),
        password: Some("secure_password_123".to_string()),
        invite_code: None,
        did: None,
        verification_code: None,
        verification_phone: None,
        recovery_key: None,
        plc_op: None,
    };

    let alice_account = auth_manager.create_account(alice_create).await.unwrap();
    println!("✅ Created account: {} ({})", alice_account.handle, alice_account.did);

    // Step 2: Create a post record
    let post_record = json!({
        "$type": "app.bsky.feed.post",
        "text": "Hello from the Rust ATProto PDS! 🦀",
        "createdAt": chrono::Utc::now().to_rfc3339(),
        "langs": ["en"]
    });

    println!("✅ Created post record");

    // Step 3: Validate the record against lexicon schema
    let post_schema_json = r#"{
        "lexicon": 1,
        "id": "app.bsky.feed.post",
        "defs": {
            "main": {
                "type": "object",
                "required": ["text", "createdAt"],
                "properties": {
                    "text": {
                        "type": "string",
                        "maxLength": 3000,
                        "maxGraphemes": 300
                    },
                    "createdAt": {
                        "type": "string",
                        "format": "datetime"
                    },
                    "langs": {
                        "type": "array",
                        "maxLength": 3,
                        "items": {
                            "type": "string",
                            "format": "language"
                        }
                    }
                }
            }
        }
    }"#;

    let post_schema = LexiconSchema::from_json(post_schema_json).unwrap();
    post_schema.validate(&post_record).unwrap();

    println!("✅ Validated post against lexicon schema");

    // Step 4: Store record in repository
    let did = atproto::types::Did::new(&alice_account.did).unwrap();
    let mut repo = Repository::create(did);

    let record_cid = repo.put_record(
        "app.bsky.feed.post",
        "test-post-1",
        post_record.clone(),
    );

    println!("✅ Stored record in repository with CID: {}", record_cid);

    // Step 5: Verify we can retrieve the record
    let retrieved = repo.get_record("app.bsky.feed.post", "test-post-1");
    assert_eq!(retrieved, Some(post_record));

    println!("✅ Retrieved record from repository");

    // Step 6: Export repository as CAR file
    let car_bytes = repo.export_car();
    println!("✅ Exported repository as CAR ({} bytes)", car_bytes.len());

    // Verify CAR has content
    assert!(car_bytes.len() > 100, "CAR file should have substantial content");

    println!("\n🎉 Complete PDS workflow successful!");
    println!("   - Account created");
    println!("   - Post validated");
    println!("   - Record stored");
    println!("   - Repository exported");
}

/// Test multi-user scenario with posts and replies
#[tokio::test]
async fn test_multi_user_pds_scenario() {
    let mut auth_manager = AuthManager::new();

    // Create two users
    let alice = auth_manager.create_account(AccountCreate {
        handle: "alice.test.com".to_string(),
        email: None,
        password: Some("password123".to_string()),
        invite_code: None,
        did: None,
        verification_code: None,
        verification_phone: None,
        recovery_key: None,
        plc_op: None,
    }).await.unwrap();

    let bob = auth_manager.create_account(AccountCreate {
        handle: "bob.test.com".to_string(),
        email: None,
        password: Some("password456".to_string()),
        invite_code: None,
        did: None,
        verification_code: None,
        verification_phone: None,
        recovery_key: None,
        plc_op: None,
    }).await.unwrap();

    println!("✅ Created 2 users: alice and bob");

    // Create Alice's repository
    let alice_did = atproto::types::Did::new(&alice.did).unwrap();
    let mut alice_repo = Repository::create(alice_did);

    // Alice creates a post
    let alice_post = json!({
        "$type": "app.bsky.feed.post",
        "text": "Just set up my PDS! 🎉",
        "createdAt": chrono::Utc::now().to_rfc3339(),
    });

    let alice_post_cid = alice_repo.put_record(
        "app.bsky.feed.post",
        "post-1",
        alice_post,
    );

    println!("✅ Alice created post: {}", alice_post_cid);

    // Create Bob's repository
    let bob_did = atproto::types::Did::new(&bob.did).unwrap();
    let mut bob_repo = Repository::create(bob_did);

    // Bob creates a reply to Alice's post
    let bob_reply = json!({
        "$type": "app.bsky.feed.post",
        "text": "Welcome to the network, Alice!",
        "createdAt": chrono::Utc::now().to_rfc3339(),
        "reply": {
            "root": {
                "uri": format!("at://{}/app.bsky.feed.post/post-1", alice.did),
                "cid": alice_post_cid.to_string(),
            },
            "parent": {
                "uri": format!("at://{}/app.bsky.feed.post/post-1", alice.did),
                "cid": alice_post_cid.to_string(),
            }
        }
    });

    let bob_reply_cid = bob_repo.put_record(
        "app.bsky.feed.post",
        "reply-1",
        bob_reply,
    );

    println!("✅ Bob replied to Alice's post: {}", bob_reply_cid);

    // Verify both repositories
    assert_eq!(alice_repo.len(), 1);
    assert_eq!(bob_repo.len(), 1);

    println!("\n🎉 Multi-user scenario successful!");
    println!("   - 2 accounts created");
    println!("   - Post and reply created");
    println!("   - Data stored in separate repos");
}

/// Test session lifecycle with authentication
#[tokio::test]
async fn test_session_lifecycle() {
    let mut auth_manager = AuthManager::new();

    // Create account
    let created = auth_manager.create_account(AccountCreate {
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

    let initial_access_token = created.access_jwt.clone();
    let initial_refresh_token = created.refresh_jwt.clone();

    println!("✅ Account created with initial session");

    // Simulate logout (delete session)
    auth_manager.delete_session(&initial_refresh_token).await.unwrap();
    println!("✅ Logged out");

    // Login again
    let session = auth_manager.create_session(
        "test.user.com",
        "my_password",
        None,
    ).await.unwrap();

    println!("✅ Logged in again");

    // Access tokens should be different (new session)
    assert_ne!(session.access_jwt, initial_access_token);

    // Refresh the session
    let refreshed = auth_manager.refresh_session(&session.refresh_jwt).await.unwrap();
    println!("✅ Refreshed session");

    // Refreshed tokens should be different
    assert_ne!(refreshed.access_jwt, session.access_jwt);
    assert_ne!(refreshed.refresh_jwt, session.refresh_jwt);

    // But handle and DID should be the same
    assert_eq!(refreshed.handle, session.handle);
    assert_eq!(refreshed.did, session.did);

    println!("\n🎉 Session lifecycle complete!");
    println!("   - Account created");
    println!("   - Logged out");
    println!("   - Logged in");
    println!("   - Session refreshed");
}

/// Test validation with real post schema
#[tokio::test]
async fn test_validation_with_real_schema() {
    // Valid post
    let valid_post = json!({
        "$type": "app.bsky.feed.post",
        "text": "Hello world!",
        "createdAt": "2025-01-15T10:00:00Z",
        "langs": ["en", "es"]
    });

    let schema_json = r#"{
        "lexicon": 1,
        "id": "app.bsky.feed.post",
        "defs": {
            "main": {
                "type": "object",
                "required": ["text", "createdAt"],
                "properties": {
                    "$type": {
                        "type": "string",
                        "const": "app.bsky.feed.post"
                    },
                    "text": {
                        "type": "string",
                        "maxLength": 3000,
                        "maxGraphemes": 300
                    },
                    "createdAt": {
                        "type": "string",
                        "format": "datetime"
                    },
                    "langs": {
                        "type": "array",
                        "maxLength": 3,
                        "items": {
                            "type": "string",
                            "format": "language"
                        }
                    },
                    "facets": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "index": {
                                    "type": "object",
                                    "required": ["byteStart", "byteEnd"],
                                    "properties": {
                                        "byteStart": {"type": "integer", "minimum": 0},
                                        "byteEnd": {"type": "integer", "minimum": 0}
                                    }
                                },
                                "features": {
                                    "type": "array"
                                }
                            }
                        }
                    }
                }
            }
        }
    }"#;

    let schema = LexiconSchema::from_json(schema_json).unwrap();

    // Valid post should pass
    assert!(schema.validate(&valid_post).is_ok());
    println!("✅ Valid post passed validation");

    // Post with text too long should fail
    let too_long_text = "a".repeat(3001);
    let invalid_post = json!({
        "$type": "app.bsky.feed.post",
        "text": too_long_text,
        "createdAt": "2025-01-15T10:00:00Z"
    });

    assert!(schema.validate(&invalid_post).is_err());
    println!("✅ Invalid post (too long) correctly rejected");

    // Post with too many languages should fail
    let too_many_langs = json!({
        "$type": "app.bsky.feed.post",
        "text": "Hello",
        "createdAt": "2025-01-15T10:00:00Z",
        "langs": ["en", "es", "fr", "de"] // Max is 3
    });

    assert!(schema.validate(&too_many_langs).is_err());
    println!("✅ Invalid post (too many languages) correctly rejected");

    // Post missing required field should fail
    let missing_field = json!({
        "$type": "app.bsky.feed.post",
        "text": "Hello"
        // Missing createdAt
    });

    assert!(schema.validate(&missing_field).is_err());
    println!("✅ Invalid post (missing field) correctly rejected");

    println!("\n🎉 Validation tests complete!");
}

/// Test repository with multiple record types
#[tokio::test]
async fn test_multi_collection_repository() {
    let did = atproto::types::Did::new("did:plc:test123").unwrap();
    let mut repo = Repository::create(did.clone());

    // Add posts
    repo.put_record("app.bsky.feed.post", "post1", json!({
        "text": "First post",
        "createdAt": "2025-01-15T10:00:00Z"
    }));

    repo.put_record("app.bsky.feed.post", "post2", json!({
        "text": "Second post",
        "createdAt": "2025-01-15T10:05:00Z"
    }));

    println!("✅ Added 2 posts");

    // Add profile
    repo.put_record("app.bsky.actor.profile", "self", json!({
        "displayName": "Test User",
        "description": "A test profile"
    }));

    println!("✅ Added profile");

    // Add follow
    repo.put_record("app.bsky.graph.follow", "follow1", json!({
        "subject": "did:plc:other123",
        "createdAt": "2025-01-15T10:10:00Z"
    }));

    println!("✅ Added follow");

    // List records in each collection
    let posts = repo.list_records("app.bsky.feed.post");
    assert_eq!(posts.len(), 2);
    println!("✅ Found 2 posts in collection");

    let profiles = repo.list_records("app.bsky.actor.profile");
    assert_eq!(profiles.len(), 1);
    println!("✅ Found 1 profile in collection");

    let follows = repo.list_records("app.bsky.graph.follow");
    assert_eq!(follows.len(), 1);
    println!("✅ Found 1 follow in collection");

    // Total records
    assert_eq!(repo.len(), 4);
    println!("✅ Total: 4 records across 3 collections");

    // Export everything
    let car_bytes = repo.export_car();
    println!("✅ Exported complete repo as CAR ({} bytes)", car_bytes.len());

    println!("\n🎉 Multi-collection repository test complete!");
}

/// Test authentication errors and edge cases
#[tokio::test]
async fn test_auth_error_handling() {
    let mut auth_manager = AuthManager::new();

    // Create initial account
    auth_manager.create_account(AccountCreate {
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

    // Try to create account with same handle
    let duplicate = auth_manager.create_account(AccountCreate {
        handle: "existing.user.com".to_string(),
        email: None,
        password: Some("different_password".to_string()),
        invite_code: None,
        did: None,
        verification_code: None,
        verification_phone: None,
        recovery_key: None,
        plc_op: None,
    }).await;

    assert!(duplicate.is_err());
    println!("✅ Duplicate handle correctly rejected");

    // Try to login with wrong password
    let wrong_password = auth_manager.create_session(
        "existing.user.com",
        "wrong_password",
        None,
    ).await;

    assert!(wrong_password.is_err());
    println!("✅ Wrong password correctly rejected");

    // Try to login with non-existent account
    let no_account = auth_manager.create_session(
        "nonexistent.user.com",
        "password123",
        None,
    ).await;

    assert!(no_account.is_err());
    println!("✅ Non-existent account correctly rejected");

    // Try invalid handle format
    let invalid_handle = auth_manager.create_account(AccountCreate {
        handle: "invalid..handle".to_string(), // Double periods not allowed
        email: None,
        password: Some("password123".to_string()),
        invite_code: None,
        did: None,
        verification_code: None,
        verification_phone: None,
        recovery_key: None,
        plc_op: None,
    }).await;

    assert!(invalid_handle.is_err());
    println!("✅ Invalid handle format correctly rejected");

    println!("\n🎉 Auth error handling test complete!");
}

/// Test complete PDS simulation: multiple users creating content
#[tokio::test]
async fn test_complete_pds_simulation() {
    println!("\n🚀 Starting Complete PDS Simulation...\n");

    let mut auth_manager = AuthManager::new();

    // Create 3 users
    let alice = auth_manager.create_account(AccountCreate {
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

    let bob = auth_manager.create_account(AccountCreate {
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

    let charlie = auth_manager.create_account(AccountCreate {
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
    println!("   - alice.social ({})", alice.did);
    println!("   - bob.social ({})", bob.did);
    println!("   - charlie.social ({})", charlie.did);

    // Create repositories for each user
    let alice_did = atproto::types::Did::new(&alice.did).unwrap();
    let bob_did = atproto::types::Did::new(&bob.did).unwrap();
    let charlie_did = atproto::types::Did::new(&charlie.did).unwrap();

    let mut alice_repo = Repository::create(alice_did);
    let mut bob_repo = Repository::create(bob_did);
    let mut charlie_repo = Repository::create(charlie_did);

    println!("\n✅ Created repositories for all users");

    // Alice creates a profile
    alice_repo.put_record("app.bsky.actor.profile", "self", json!({
        "displayName": "Alice",
        "description": "I love Rust! 🦀"
    }));

    // Alice creates a post
    let alice_post = alice_repo.put_record("app.bsky.feed.post", "post1", json!({
        "text": "Just deployed my first PDS written in Rust! 🎉",
        "createdAt": chrono::Utc::now().to_rfc3339(),
        "langs": ["en"]
    }));

    println!("\n✅ Alice created profile and post");

    // Bob creates profile and follows Alice
    bob_repo.put_record("app.bsky.actor.profile", "self", json!({
        "displayName": "Bob",
        "description": "Developer"
    }));

    bob_repo.put_record("app.bsky.graph.follow", "follow1", json!({
        "subject": alice.did.to_string(),
        "createdAt": chrono::Utc::now().to_rfc3339()
    }));

    // Bob replies to Alice's post
    bob_repo.put_record("app.bsky.feed.post", "reply1", json!({
        "text": "That's awesome Alice! Rust is the best!",
        "createdAt": chrono::Utc::now().to_rfc3339(),
        "reply": {
            "root": {
                "uri": format!("at://{}/app.bsky.feed.post/post1", alice.did),
                "cid": alice_post.to_string()
            },
            "parent": {
                "uri": format!("at://{}/app.bsky.feed.post/post1", alice.did),
                "cid": alice_post.to_string()
            }
        }
    }));

    println!("✅ Bob created profile, followed Alice, and replied to her post");

    // Charlie creates profile and follows both
    charlie_repo.put_record("app.bsky.actor.profile", "self", json!({
        "displayName": "Charlie"
    }));

    charlie_repo.put_record("app.bsky.graph.follow", "follow1", json!({
        "subject": alice.did.to_string(),
        "createdAt": chrono::Utc::now().to_rfc3339()
    }));

    charlie_repo.put_record("app.bsky.graph.follow", "follow2", json!({
        "subject": bob.did.to_string(),
        "createdAt": chrono::Utc::now().to_rfc3339()
    }));

    // Charlie creates a post
    charlie_repo.put_record("app.bsky.feed.post", "post1", json!({
        "text": "Great to see the Rust ATProto ecosystem growing!",
        "createdAt": chrono::Utc::now().to_rfc3339()
    }));

    println!("✅ Charlie created profile, followed both users, and created a post");

    // Export all repositories
    let alice_car = alice_repo.export_car();
    let bob_car = bob_repo.export_car();
    let charlie_car = charlie_repo.export_car();

    println!("\n✅ Exported all repositories:");
    println!("   - Alice: {} bytes", alice_car.len());
    println!("   - Bob: {} bytes", bob_car.len());
    println!("   - Charlie: {} bytes", charlie_car.len());

    // Verify record counts
    println!("\n✅ Record counts:");
    println!("   - Alice: {} records", alice_repo.len());
    println!("   - Bob: {} records", bob_repo.len());
    println!("   - Charlie: {} records", charlie_repo.len());

    assert_eq!(alice_repo.len(), 2); // profile + post
    assert_eq!(bob_repo.len(), 3); // profile + follow + reply
    assert_eq!(charlie_repo.len(), 4); // profile + 2 follows + post

    println!("\n🎉 Complete PDS simulation successful!");
    println!("   ✅ 3 users created");
    println!("   ✅ Profiles created");
    println!("   ✅ Social graph (follows) established");
    println!("   ✅ Posts and replies created");
    println!("   ✅ All data exported as CAR files");
    println!("\n   This demonstrates a fully functional PDS!");
}
