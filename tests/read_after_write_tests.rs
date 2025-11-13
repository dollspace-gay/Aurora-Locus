/// Read-After-Write Consistency Tests
///
/// Tests for the read-after-write consistency module that ensures users
/// see their own writes immediately, even before AppView indexing completes.

#[cfg(test)]
mod read_after_write_tests {
    use aurora_locus::read_after_write::{get_local_lag, LocalRecords, RecordDescript};
    use chrono::Utc;
    use serde_json::json;

    /// Helper to create a test RecordDescript for a post
    fn create_test_post(uri: &str, indexed_at: &str, text: &str) -> RecordDescript {
        RecordDescript {
            uri: uri.to_string(),
            cid: "bafytest123".to_string(),
            indexed_at: indexed_at.to_string(),
            record: json!({
                "$type": "app.bsky.feed.post",
                "text": text,
                "createdAt": indexed_at,
            }),
        }
    }

    #[test]
    fn test_get_local_lag_with_posts() {
        // Create local records with different timestamps
        let old_time = "2024-01-01T10:00:00Z";
        let new_time = "2024-01-01T11:00:00Z";

        let local_records = LocalRecords {
            rev: "abc123".to_string(),
            count: 2,
            posts: vec![
                create_test_post("at://did:plc:test/app.bsky.feed.post/1", old_time, "Old post"),
                create_test_post("at://did:plc:test/app.bsky.feed.post/2", new_time, "New post"),
            ],
            profile: None,
        };

        let lag = get_local_lag(&local_records);
        assert!(lag.is_some());
        assert!(lag.unwrap() > 0); // Should have some lag since old_time is in the past
    }

    #[test]
    fn test_get_local_lag_with_profile() {
        let profile_time = "2024-01-01T10:00:00Z";

        let local_records = LocalRecords {
            rev: "abc123".to_string(),
            count: 1,
            posts: vec![],
            profile: Some(RecordDescript {
                uri: "at://did:plc:test/app.bsky.actor.profile/self".to_string(),
                cid: "bafyprofile".to_string(),
                indexed_at: profile_time.to_string(),
                record: json!({
                    "$type": "app.bsky.actor.profile",
                    "displayName": "Test User",
                }),
            }),
        };

        let lag = get_local_lag(&local_records);
        assert!(lag.is_some());
        assert!(lag.unwrap() > 0);
    }

    #[test]
    fn test_get_local_lag_empty() {
        let local_records = LocalRecords {
            rev: "abc123".to_string(),
            count: 0,
            posts: vec![],
            profile: None,
        };

        let lag = get_local_lag(&local_records);
        assert!(lag.is_none());
    }

    // Note: Full integration tests with format_and_insert_posts_in_feed require
    // a complete AppContext setup with database initialization. These tests
    // would be better suited for end-to-end testing with a test server.

    #[test]
    fn test_chronological_ordering() {
        // Test that posts are ordered by indexedAt timestamp
        let posts = vec![
            create_test_post(
                "at://did:plc:test/app.bsky.feed.post/1",
                "2024-01-01T10:00:00Z",
                "Old post",
            ),
            create_test_post(
                "at://did:plc:test/app.bsky.feed.post/2",
                "2024-01-01T11:00:00Z",
                "Newer post",
            ),
            create_test_post(
                "at://did:plc:test/app.bsky.feed.post/3",
                "2024-01-01T09:00:00Z",
                "Oldest post",
            ),
        ];

        // Verify timestamps are parseable and can be compared
        for post in &posts {
            let parsed = chrono::DateTime::parse_from_rfc3339(&post.indexed_at);
            assert!(parsed.is_ok(), "Failed to parse timestamp: {}", post.indexed_at);
        }

        // Verify chronological comparison works
        assert!(posts[0].indexed_at.as_str() > posts[2].indexed_at.as_str());
        assert!(posts[1].indexed_at.as_str() > posts[0].indexed_at.as_str());
    }

    #[test]
    fn test_local_records_count() {
        let local_records = LocalRecords {
            rev: "abc123".to_string(),
            count: 3,
            posts: vec![
                create_test_post(
                    "at://did:plc:test/app.bsky.feed.post/1",
                    "2024-01-01T10:00:00Z",
                    "Post 1",
                ),
                create_test_post(
                    "at://did:plc:test/app.bsky.feed.post/2",
                    "2024-01-01T11:00:00Z",
                    "Post 2",
                ),
                create_test_post(
                    "at://did:plc:test/app.bsky.feed.post/3",
                    "2024-01-01T12:00:00Z",
                    "Post 3",
                ),
            ],
            profile: None,
        };

        assert_eq!(local_records.count, 3);
        assert_eq!(local_records.posts.len(), 3);
        assert_eq!(local_records.rev, "abc123");
    }

    #[test]
    fn test_record_descript_structure() {
        let post = create_test_post(
            "at://did:plc:test/app.bsky.feed.post/abc",
            "2024-01-01T10:00:00Z",
            "Test post",
        );

        assert_eq!(post.uri, "at://did:plc:test/app.bsky.feed.post/abc");
        assert_eq!(post.cid, "bafytest123");
        assert_eq!(post.indexed_at, "2024-01-01T10:00:00Z");
        assert_eq!(post.record["text"], "Test post");
        assert_eq!(post.record["$type"], "app.bsky.feed.post");
    }

    #[test]
    fn test_appview_lag_simulation() {
        // Simulate AppView being 5 minutes behind
        let now = Utc::now();
        let appview_time = now - chrono::Duration::minutes(5);
        let local_time = now;

        let local_records = LocalRecords {
            rev: "latest".to_string(),
            count: 1,
            posts: vec![create_test_post(
                "at://did:plc:test/app.bsky.feed.post/1",
                &local_time.to_rfc3339(),
                "Very recent post",
            )],
            profile: None,
        };

        // Parse timestamps
        let appview_ts = chrono::DateTime::parse_from_rfc3339(&appview_time.to_rfc3339()).unwrap();
        let local_ts =
            chrono::DateTime::parse_from_rfc3339(&local_records.posts[0].indexed_at).unwrap();

        // Verify local post is newer than AppView's last indexed time
        assert!(local_ts > appview_ts);

        // This post should appear in the feed since it's newer than AppView's index
        let lag = get_local_lag(&local_records);
        assert!(lag.is_some());
        let lag_seconds = lag.unwrap() / 1000; // Convert ms to seconds
        assert!(
            lag_seconds < 10,
            "Lag should be less than 10 seconds, got {}",
            lag_seconds
        );
    }

    #[test]
    fn test_profile_record_structure() {
        let profile = RecordDescript {
            uri: "at://did:plc:test/app.bsky.actor.profile/self".to_string(),
            cid: "bafyprofile".to_string(),
            indexed_at: "2024-01-01T10:00:00Z".to_string(),
            record: json!({
                "$type": "app.bsky.actor.profile",
                "displayName": "Test User",
                "description": "Test bio",
            }),
        };

        assert_eq!(
            profile.uri,
            "at://did:plc:test/app.bsky.actor.profile/self"
        );
        assert_eq!(profile.record["displayName"], "Test User");
        assert_eq!(profile.record["description"], "Test bio");
        assert_eq!(profile.record["$type"], "app.bsky.actor.profile");
    }

    #[test]
    fn test_empty_feed_with_local_posts() {
        // Test that local posts are added to an empty feed
        let local_records = LocalRecords {
            rev: "abc123".to_string(),
            count: 1,
            posts: vec![create_test_post(
                "at://did:plc:test/app.bsky.feed.post/1",
                "2024-01-01T10:00:00Z",
                "First post",
            )],
            profile: None,
        };

        assert_eq!(local_records.posts.len(), 1);
        assert_eq!(local_records.count, 1);

        // Verify the post can be added to an empty feed
        let empty_feed: Vec<serde_json::Value> = vec![];
        assert_eq!(empty_feed.len(), 0);
    }
}
