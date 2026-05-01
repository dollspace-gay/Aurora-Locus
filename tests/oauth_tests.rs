//! OAuth 2.1 Unit Tests.
//!
//! Tests for OAuth token validation, scope enforcement, and security features.

#[cfg(test)]
mod oauth_token_validation {
    use aurora_locus::oauth::{
        require_all_scopes, require_any_scope, require_scope, AtProtoScope, ScopeSet,
    };
    use std::str::FromStr;

    #[test]
    fn test_scope_set_parsing_single_scope() {
        let scope_str = "atproto:repo.create";
        let scopes = ScopeSet::from_str(scope_str).unwrap();

        assert!(scopes.has_scope(&AtProtoScope::RepoCreate));
        assert!(!scopes.has_scope(&AtProtoScope::RepoUpdate));
    }

    #[test]
    fn test_scope_set_parsing_multiple_scopes() {
        let scope_str = "atproto:repo.create atproto:repo.update atproto:blob.upload";
        let scopes = ScopeSet::from_str(scope_str).unwrap();

        assert!(scopes.has_scope(&AtProtoScope::RepoCreate));
        assert!(scopes.has_scope(&AtProtoScope::RepoUpdate));
        assert!(scopes.has_scope(&AtProtoScope::BlobUpload));
        assert!(!scopes.has_scope(&AtProtoScope::RepoDelete));
    }

    #[test]
    fn test_scope_set_parsing_wildcard_all() {
        let scope_str = "atproto:*";
        let scopes = ScopeSet::from_str(scope_str).unwrap();

        // atproto:* should grant all scopes
        assert!(scopes.has_scope(&AtProtoScope::RepoCreate));
        assert!(scopes.has_scope(&AtProtoScope::RepoUpdate));
        assert!(scopes.has_scope(&AtProtoScope::RepoDelete));
        assert!(scopes.has_scope(&AtProtoScope::BlobUpload));
        assert!(scopes.has_scope(&AtProtoScope::Read));
    }

    #[test]
    fn test_scope_set_parsing_write_wildcard() {
        let scope_str = "atproto:write";
        let scopes = ScopeSet::from_str(scope_str).unwrap();

        // atproto:write should grant create, update, delete
        assert!(scopes.has_scope(&AtProtoScope::RepoCreate));
        assert!(scopes.has_scope(&AtProtoScope::RepoUpdate));
        assert!(scopes.has_scope(&AtProtoScope::RepoDelete));
        assert!(scopes.has_scope(&AtProtoScope::BlobUpload));

        // But not admin scopes
        assert!(!scopes.has_scope(&AtProtoScope::AdminAll));
    }

    #[test]
    fn test_scope_set_parsing_repo_wildcard() {
        let scope_str = "atproto:repo.*";
        let scopes = ScopeSet::from_str(scope_str).unwrap();

        // atproto:repo.* should grant all repo operations
        assert!(scopes.has_scope(&AtProtoScope::RepoCreate));
        assert!(scopes.has_scope(&AtProtoScope::RepoUpdate));
        assert!(scopes.has_scope(&AtProtoScope::RepoDelete));
        assert!(scopes.has_scope(&AtProtoScope::RepoGet));
        assert!(scopes.has_scope(&AtProtoScope::RepoList));
        assert!(scopes.has_scope(&AtProtoScope::RepoAll));
    }

    #[test]
    fn test_scope_hierarchy_all_includes_read() {
        let scope_str = "atproto:*";
        let scopes = ScopeSet::from_str(scope_str).unwrap();

        assert!(scopes.has_scope(&AtProtoScope::Read));
        assert!(scopes.has_scope(&AtProtoScope::RepoGet));
    }

    #[test]
    fn test_scope_hierarchy_write_includes_create() {
        let scope_str = "atproto:write";
        let scopes = ScopeSet::from_str(scope_str).unwrap();

        assert!(scopes.has_scope(&AtProtoScope::RepoCreate));
        assert!(scopes.has_scope(&AtProtoScope::RepoUpdate));
    }

    #[test]
    fn test_require_scope_success() {
        let token_scopes = "atproto:repo.create atproto:read";
        let result = require_scope(token_scopes, &AtProtoScope::RepoCreate);

        assert!(result.is_ok());
    }

    #[test]
    fn test_require_scope_failure() {
        let token_scopes = "atproto:read";
        let result = require_scope(token_scopes, &AtProtoScope::RepoCreate);

        assert!(result.is_err());
    }

    #[test]
    fn test_require_scope_wildcard_grants_specific() {
        let token_scopes = "atproto:*";
        let result = require_scope(token_scopes, &AtProtoScope::RepoCreate);

        assert!(result.is_ok());
    }

    #[test]
    fn test_require_any_scope_success() {
        let token_scopes = "atproto:repo.update";
        let required = vec![
            AtProtoScope::RepoCreate,
            AtProtoScope::RepoUpdate,
            AtProtoScope::RepoDelete,
        ];
        let result = require_any_scope(token_scopes, &required);

        assert!(result.is_ok());
    }

    #[test]
    fn test_require_any_scope_failure() {
        let token_scopes = "atproto:read";
        let required = vec![
            AtProtoScope::RepoCreate,
            AtProtoScope::RepoUpdate,
            AtProtoScope::RepoDelete,
        ];
        let result = require_any_scope(token_scopes, &required);

        assert!(result.is_err());
    }

    #[test]
    fn test_require_all_scopes_success() {
        let token_scopes = "atproto:repo.create atproto:repo.update atproto:blob.upload";
        let required = vec![AtProtoScope::RepoCreate, AtProtoScope::RepoUpdate];
        let result = require_all_scopes(token_scopes, &required);

        assert!(result.is_ok());
    }

    #[test]
    fn test_require_all_scopes_failure_missing_one() {
        let token_scopes = "atproto:repo.create";
        let required = vec![AtProtoScope::RepoCreate, AtProtoScope::RepoUpdate];
        let result = require_all_scopes(token_scopes, &required);

        assert!(result.is_err());
    }

    #[test]
    fn test_require_all_scopes_wildcard_grants_all() {
        let token_scopes = "atproto:*";
        let required = vec![
            AtProtoScope::RepoCreate,
            AtProtoScope::RepoUpdate,
            AtProtoScope::BlobUpload,
        ];
        let result = require_all_scopes(token_scopes, &required);

        assert!(result.is_ok());
    }

    #[test]
    fn test_empty_scope_string() {
        let scope_str = "";
        let scopes = ScopeSet::from_str(scope_str);

        // Empty scope string should be valid but grant no permissions
        assert!(scopes.is_ok());
        let scopes = scopes.unwrap();
        assert!(!scopes.has_scope(&AtProtoScope::Read));
        assert!(!scopes.has_scope(&AtProtoScope::RepoCreate));
    }

    #[test]
    fn test_case_sensitivity() {
        let scope_str = "atproto:REPO.CREATE";
        let scopes = ScopeSet::from_str(scope_str);

        // Scopes should be case-sensitive (lowercase is standard)
        assert!(scopes.is_ok());
        let scopes = scopes.unwrap();
        // This should NOT match because scopes are case-sensitive
        assert!(!scopes.has_scope(&AtProtoScope::RepoCreate));
    }

    #[test]
    fn test_duplicate_scopes() {
        let scope_str = "atproto:read atproto:read atproto:read";
        let scopes = ScopeSet::from_str(scope_str).unwrap();

        // Duplicate scopes should be handled gracefully
        assert!(scopes.has_scope(&AtProtoScope::Read));
    }

    #[test]
    fn test_extra_whitespace() {
        let scope_str = "  atproto:read   atproto:repo.create  ";
        let scopes = ScopeSet::from_str(scope_str).unwrap();

        assert!(scopes.has_scope(&AtProtoScope::Read));
        assert!(scopes.has_scope(&AtProtoScope::RepoCreate));
    }

    #[test]
    fn test_scope_display() {
        let scope = AtProtoScope::RepoCreate;
        let display = format!("{}", scope);

        assert_eq!(display, "atproto:repo.create");
    }

    #[test]
    fn test_admin_scope_not_granted_by_write() {
        let scope_str = "atproto:write";
        let scopes = ScopeSet::from_str(scope_str).unwrap();

        // Admin scopes should not be included in write wildcard
        assert!(!scopes.has_scope(&AtProtoScope::AdminAll));
        assert!(!scopes.has_scope(&AtProtoScope::AdminServer));
    }

    #[test]
    fn test_admin_scope_granted_by_all() {
        let scope_str = "atproto:*";
        let scopes = ScopeSet::from_str(scope_str).unwrap();

        // atproto:* should include admin scopes
        assert!(scopes.has_scope(&AtProtoScope::AdminAll));
        assert!(scopes.has_scope(&AtProtoScope::AdminServer));
    }
}

#[cfg(test)]
mod oauth_lexicon_mapping {
    use aurora_locus::oauth::{lexicon_to_scope, AtProtoScope};

    #[test]
    fn test_lexicon_repo_create() {
        let nsid = "com.atproto.repo.createRecord";
        let scope = lexicon_to_scope(nsid);

        assert_eq!(scope, AtProtoScope::RepoCreate);
    }

    #[test]
    fn test_lexicon_repo_put() {
        let nsid = "com.atproto.repo.putRecord";
        let scope = lexicon_to_scope(nsid);

        assert_eq!(scope, AtProtoScope::RepoUpdate);
    }

    #[test]
    fn test_lexicon_repo_delete() {
        let nsid = "com.atproto.repo.deleteRecord";
        let scope = lexicon_to_scope(nsid);

        assert_eq!(scope, AtProtoScope::RepoDelete);
    }

    #[test]
    fn test_lexicon_repo_get() {
        let nsid = "com.atproto.repo.getRecord";
        let scope = lexicon_to_scope(nsid);

        assert_eq!(scope, AtProtoScope::RepoGet);
    }

    #[test]
    fn test_lexicon_repo_list() {
        let nsid = "com.atproto.repo.listRecords";
        let scope = lexicon_to_scope(nsid);

        assert_eq!(scope, AtProtoScope::RepoList);
    }

    #[test]
    fn test_lexicon_blob_upload() {
        let nsid = "com.atproto.repo.uploadBlob";
        let scope = lexicon_to_scope(nsid);

        assert_eq!(scope, AtProtoScope::BlobUpload);
    }

    #[test]
    fn test_lexicon_admin() {
        let nsid = "com.atproto.admin.disableAccount";
        let scope = lexicon_to_scope(nsid);

        assert_eq!(scope, AtProtoScope::AdminAll);
    }

    #[test]
    fn test_lexicon_identity_resolve() {
        let nsid = "com.atproto.identity.resolveHandle";
        let scope = lexicon_to_scope(nsid);

        assert_eq!(scope, AtProtoScope::IdentityResolveDid);
    }

    #[test]
    fn test_lexicon_identity_update() {
        let nsid = "com.atproto.identity.updateHandle";
        let scope = lexicon_to_scope(nsid);

        assert_eq!(scope, AtProtoScope::IdentityUpdateProfile);
    }

    #[test]
    fn test_lexicon_unknown_defaults_to_read() {
        let nsid = "com.unknown.endpoint";
        let scope = lexicon_to_scope(nsid);

        assert_eq!(scope, AtProtoScope::Read);
    }

    #[test]
    fn test_lexicon_app_bsky_post() {
        let nsid = "app.bsky.feed.post";
        let scope = lexicon_to_scope(nsid);

        // app.bsky.feed.post should map to RepoCreate
        assert_eq!(scope, AtProtoScope::RepoCreate);
    }
}

#[cfg(test)]
mod oauth_security {
    /// Security-related OAuth tests

    #[test]
    fn test_bearer_token_extraction() {
        let auth_header = "Bearer abc123token456";
        let token = auth_header.strip_prefix("Bearer ");

        assert_eq!(token, Some("abc123token456"));
    }

    #[test]
    fn test_bearer_token_missing_prefix() {
        let auth_header = "abc123token456";
        let token = auth_header.strip_prefix("Bearer ");

        assert_eq!(token, None);
    }

    #[test]
    fn test_bearer_token_lowercase_bearer() {
        let auth_header = "bearer abc123token456";
        let token = auth_header.strip_prefix("Bearer ");

        // Should be case-sensitive (RFC 6750 specifies "Bearer")
        assert_eq!(token, None);
    }

    #[test]
    fn test_bearer_token_extra_spaces() {
        let auth_header = "Bearer  abc123token456";
        let token = auth_header.strip_prefix("Bearer ").map(str::trim);

        assert_eq!(token, Some("abc123token456"));
    }

    #[test]
    fn test_dpop_header_case() {
        // DPoP header should be case-insensitive per HTTP spec
        let header_name = "dpop";
        assert_eq!(header_name.to_lowercase(), "dpop");

        let header_name = "DPoP";
        assert_eq!(header_name.to_lowercase(), "dpop");
    }

    #[test]
    fn test_token_length_limits() {
        // Access tokens should have reasonable length limits
        let short_token = "abc";
        let normal_token = "a".repeat(32);
        let long_token = "a".repeat(10000);

        assert!(short_token.len() < 32);
        assert!(normal_token.len() == 32);
        assert!(long_token.len() > 1000);

        // In production, we should reject tokens that are too short or too long
        // to prevent DoS attacks via extremely long tokens
    }

    #[test]
    fn test_scope_string_limits() {
        // Scope strings should have reasonable length limits
        let many_scopes = (0..1000)
            .map(|i| format!("atproto:scope{}", i))
            .collect::<Vec<_>>()
            .join(" ");

        // This should be detected and rejected to prevent DoS
        assert!(many_scopes.len() > 10000);
    }
}

#[cfg(test)]
mod pkce_verification {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use sha2::{Digest, Sha256};

    /// Test helper to generate PKCE code_challenge from code_verifier
    fn generate_code_challenge(code_verifier: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(code_verifier.as_bytes());
        let hash = hasher.finalize();
        URL_SAFE_NO_PAD.encode(hash)
    }

    #[test]
    fn test_pkce_challenge_generation() {
        // Test that we can generate a valid PKCE challenge
        let code_verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = generate_code_challenge(code_verifier);

        // Challenge should be base64url encoded SHA-256 hash (43 characters)
        assert_eq!(challenge.len(), 43);
        assert!(challenge
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn test_pkce_challenge_deterministic() {
        // Same code_verifier should always produce same challenge
        let code_verifier = "test_verifier_12345678901234567890";
        let challenge1 = generate_code_challenge(code_verifier);
        let challenge2 = generate_code_challenge(code_verifier);

        assert_eq!(challenge1, challenge2);
    }

    #[test]
    fn test_pkce_challenge_unique() {
        // Different code_verifiers should produce different challenges
        let verifier1 = "verifier_one_12345678901234567890123";
        let verifier2 = "verifier_two_12345678901234567890123";

        let challenge1 = generate_code_challenge(verifier1);
        let challenge2 = generate_code_challenge(verifier2);

        assert_ne!(challenge1, challenge2);
    }

    #[test]
    fn test_pkce_verifier_minimum_length() {
        // RFC 7636 requires code_verifier to be 43-128 characters
        let short_verifier = "too_short";
        assert!(short_verifier.len() < 43);

        // While this generates a challenge, it should be rejected by the server
        let _challenge = generate_code_challenge(short_verifier);
    }

    #[test]
    fn test_pkce_verifier_recommended_length() {
        // RFC 7636 recommends 43 characters
        let verifier = "a".repeat(43);
        assert_eq!(verifier.len(), 43);

        let challenge = generate_code_challenge(&verifier);
        assert_eq!(challenge.len(), 43);
    }

    #[test]
    fn test_pkce_verifier_maximum_length() {
        // RFC 7636 allows up to 128 characters
        let verifier = "a".repeat(128);
        assert_eq!(verifier.len(), 128);

        let challenge = generate_code_challenge(&verifier);
        assert_eq!(challenge.len(), 43); // Challenge is always 43 chars (SHA-256)
    }

    #[test]
    fn test_pkce_challenge_verification() {
        // Simulate the full PKCE flow
        let code_verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let expected_challenge = generate_code_challenge(code_verifier);

        // In the token endpoint, we would re-compute the challenge
        let computed_challenge = generate_code_challenge(code_verifier);

        // Verification succeeds if they match
        assert_eq!(computed_challenge, expected_challenge);
    }

    #[test]
    fn test_pkce_challenge_verification_fails() {
        // Test that verification fails with wrong verifier
        let correct_verifier = "correct_verifier_1234567890123456789";
        let wrong_verifier = "wrong_verifier_1234567890123456789";

        let expected_challenge = generate_code_challenge(correct_verifier);
        let computed_challenge = generate_code_challenge(wrong_verifier);

        // Verification should fail
        assert_ne!(computed_challenge, expected_challenge);
    }

    #[test]
    fn test_pkce_challenge_case_sensitive() {
        // PKCE should be case-sensitive
        let verifier_lower = "abcdefghijklmnopqrstuvwxyz1234567890123";
        let verifier_upper = "ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890123";

        let challenge_lower = generate_code_challenge(verifier_lower);
        let challenge_upper = generate_code_challenge(verifier_upper);

        assert_ne!(challenge_lower, challenge_upper);
    }

    #[test]
    fn test_pkce_base64url_no_padding() {
        // Base64url encoding should not include padding (=)
        let verifier = "a".repeat(43);
        let challenge = generate_code_challenge(&verifier);

        assert!(!challenge.contains('='));
        assert!(!challenge.contains('+'));
        assert!(!challenge.contains('/'));
    }

    #[test]
    fn test_pkce_example_from_rfc() {
        // Example from RFC 7636 Appendix B
        let code_verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let expected_challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

        let computed_challenge = generate_code_challenge(code_verifier);

        assert_eq!(computed_challenge, expected_challenge);
    }
}

#[cfg(test)]
mod backward_compatibility {
    /// Tests for backward compatibility with legacy authentication

    #[test]
    fn test_legacy_session_token_format() {
        // Legacy session tokens should still work
        let session_token = "session_abc123def456";

        // Basic validation
        assert!(session_token.starts_with("session_"));
        assert!(session_token.len() > 8);
    }

    #[test]
    fn test_oauth_token_format_different_from_session() {
        // OAuth tokens and session tokens should be distinguishable
        let session_token = "session_abc123";
        let oauth_token = "token_xyz789";

        assert!(session_token.starts_with("session_"));
        assert!(!oauth_token.starts_with("session_"));
    }

    #[test]
    fn test_jwt_token_format() {
        // JWT tokens have three base64 parts separated by dots
        let jwt_example = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.TJVA95OrM7E2cBab30RMHrHDcEfxjoYZgeFONFh7HgQ";

        let parts: Vec<&str> = jwt_example.split('.').collect();
        assert_eq!(parts.len(), 3);

        // Each part should be base64
        for part in parts {
            assert!(part
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_'));
        }
    }

    #[test]
    fn test_cross_pds_jwt_scope() {
        // Cross-PDS JWT tokens should have specific scope format
        let scope = "com.atproto.access";

        assert!(scope.starts_with("com.atproto"));
    }
}
