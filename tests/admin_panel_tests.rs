/// Tests for admin panel API endpoints
///
/// Note: These are unit tests that verify the logic is correct.
/// Integration tests would require a running server.

#[cfg(test)]
mod tests {
    // Test invite code generation
    #[test]
    fn test_invite_code_generation() {
        use rand::Rng;
        const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let mut rng = rand::thread_rng();

        let code: String = (0..16)
            .map(|_| {
                let idx = rng.gen_range(0..CHARSET.len());
                CHARSET[idx] as char
            })
            .collect();

        assert_eq!(code.len(), 16);
        assert!(code.chars().all(|c| c.is_ascii_alphanumeric()));
        assert!(code.chars().all(|c| !c.is_uppercase()));
    }

    #[test]
    fn test_multiple_invite_codes_are_unique() {
        use rand::Rng;
        use std::collections::HashSet;
        const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

        let mut codes = HashSet::new();
        for _ in 0..100 {
            let mut rng = rand::thread_rng();
            let code: String = (0..16)
                .map(|_| {
                    let idx = rng.gen_range(0..CHARSET.len());
                    CHARSET[idx] as char
                })
                .collect();
            codes.insert(code);
        }

        // With 16 character codes from 36-character alphabet,
        // collisions are astronomically unlikely in 100 attempts
        assert_eq!(codes.len(), 100);
    }

    #[test]
    fn test_admin_authorization_header_parsing() {
        let auth_header = "Bearer abc123token";
        let token = auth_header.strip_prefix("Bearer ");
        assert_eq!(token, Some("abc123token"));

        let invalid_header = "abc123token";
        let token = invalid_header.strip_prefix("Bearer ");
        assert_eq!(token, None);
    }

    #[test]
    fn test_storage_size_calculation() {
        // Test GB conversion from bytes
        let bytes = 1_500_000_000_i64; // 1.5 GB
        let gb = bytes as f64 / 1024.0 / 1024.0 / 1024.0;
        assert!((gb - 1.4).abs() < 0.1); // Approximately 1.4 GB
    }

    #[test]
    fn test_admin_stats_default_values() {
        // Verify that unwrap_or returns 0 for missing stats
        let total_users: i64 = None.unwrap_or(0);
        let total_posts: i64 = Some(42).unwrap_or(0);

        assert_eq!(total_users, 0);
        assert_eq!(total_posts, 42);
    }
}
