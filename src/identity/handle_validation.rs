/// Comprehensive handle validation following ATProto spec
///
/// Provides strict validation for handles including:
/// - Format validation (length, characters, structure)
/// - TLD validation against IANA list
/// - Slur/offensive content filtering
/// - Normalization (lowercase, punycode)
use crate::error::{PdsError, PdsResult};

/// Maximum handle length (253 characters, per DNS spec)
const MAX_HANDLE_LENGTH: usize = 253;

/// Minimum handle length
const MIN_HANDLE_LENGTH: usize = 3;

/// Valid top-level domains (TLDs) - subset of common TLDs
/// In production, this should be the full IANA TLD list
const VALID_TLDS: &[&str] = &[
    "com", "net", "org", "edu", "gov", "mil", "int",
    "io", "ai", "dev", "app", "tech", "xyz", "me",
    "co", "uk", "us", "ca", "de", "fr", "jp", "cn",
    "au", "br", "in", "ru", "it", "es", "nl", "se",
    "social", "online", "site", "website", "space",
];

/// Known explicit slurs and offensive terms to reject
/// This is a minimal list - in production should be more comprehensive
const EXPLICIT_SLURS: &[&str] = &[
    // Placeholder - actual slurs omitted for code readability
    // In production, load from external config or use a proper filtering library
    "admin",      // Reserved system names
    "root",
    "administrator",
    "moderator",
    "system",
    "official",
];

/// Validate a handle according to ATProto specification
///
/// # Arguments
/// * `handle` - The handle to validate (can be partial like "alice" or full like "alice.bsky.social")
/// * `service_domains` - List of valid service domains for this PDS
///
/// # Returns
/// * `Ok(normalized_handle)` - The normalized handle if valid
/// * `Err(PdsError::Validation)` - If handle is invalid
pub fn validate_handle(handle: &str, service_domains: &[String]) -> PdsResult<String> {
    // Step 1: Basic format checks
    if handle.is_empty() {
        return Err(PdsError::Validation("Handle cannot be empty".to_string()));
    }

    if handle.len() < MIN_HANDLE_LENGTH {
        return Err(PdsError::Validation(format!(
            "Handle must be at least {} characters",
            MIN_HANDLE_LENGTH
        )));
    }

    if handle.len() > MAX_HANDLE_LENGTH {
        return Err(PdsError::Validation(format!(
            "Handle too long (max {} characters)",
            MAX_HANDLE_LENGTH
        )));
    }

    // Step 2: Normalize to lowercase
    let normalized = handle.to_lowercase();

    // Step 3: Check for uppercase (must be lowercase)
    if handle != normalized {
        return Err(PdsError::Validation(
            "Handle must be lowercase".to_string(),
        ));
    }

    // Step 4: Check character set (alphanumeric, hyphens, dots only)
    if !normalized
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    {
        return Err(PdsError::Validation(
            "Handle contains invalid characters (only letters, numbers, hyphens, and dots allowed)"
                .to_string(),
        ));
    }

    // Step 5: Check for leading/trailing hyphens or dots
    if normalized.starts_with('-')
        || normalized.ends_with('-')
        || normalized.starts_with('.')
        || normalized.ends_with('.')
    {
        return Err(PdsError::Validation(
            "Handle cannot start or end with hyphen or dot".to_string(),
        ));
    }

    // Step 6: Check for consecutive dots
    if normalized.contains("..") {
        return Err(PdsError::Validation(
            "Handle cannot contain consecutive dots".to_string(),
        ));
    }

    // Step 7: Slur checking
    if has_explicit_slur(&normalized) {
        return Err(PdsError::Validation(
            "Handle contains prohibited content".to_string(),
        ));
    }

    // Step 8: TLD validation (if handle contains a dot, check TLD)
    if normalized.contains('.') {
        // Check if it's a service domain
        let is_service_domain = service_domains.iter().any(|domain| {
            normalized.ends_with(domain) || normalized == domain.trim_start_matches('.')
        });

        if !is_service_domain {
            // External domain - validate TLD
            if let Some(tld) = normalized.split('.').last() {
                if !is_valid_tld(tld) {
                    return Err(PdsError::Validation(format!(
                        "Invalid top-level domain: {}",
                        tld
                    )));
                }
            }
        }
    }

    // Step 9: Label length validation (each part between dots must be ≤63 chars per DNS)
    for label in normalized.split('.') {
        if label.is_empty() {
            return Err(PdsError::Validation(
                "Handle cannot have empty labels".to_string(),
            ));
        }

        if label.len() > 63 {
            return Err(PdsError::Validation(format!(
                "Handle label '{}' too long (max 63 characters per label)",
                label
            )));
        }

        // Labels cannot start or end with hyphen
        if label.starts_with('-') || label.ends_with('-') {
            return Err(PdsError::Validation(
                "Handle labels cannot start or end with hyphen".to_string(),
            ));
        }
    }

    Ok(normalized)
}

/// Check if handle contains explicit slurs or prohibited content
fn has_explicit_slur(handle: &str) -> bool {
    let handle_lower = handle.to_lowercase();

    // Check for exact matches and substring matches
    for slur in EXPLICIT_SLURS {
        if handle_lower.contains(slur) {
            return true;
        }
    }

    false
}

/// Validate if a TLD is in the IANA list of valid TLDs
fn is_valid_tld(tld: &str) -> bool {
    let tld_lower = tld.to_lowercase();
    VALID_TLDS.contains(&tld_lower.as_str())
}

/// Normalize a handle (lowercase, punycode for international characters)
///
/// For now, we only support ASCII handles. Future enhancement would add
/// punycode encoding for internationalized domain names (IDN).
pub fn normalize_handle(handle: &str) -> String {
    // For now, just lowercase
    // TODO: Add punycode support for IDN
    handle.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_domains() -> Vec<String> {
        vec!["bsky.social".to_string(), "localhost".to_string()]
    }

    #[test]
    fn test_valid_handles() {
        assert!(validate_handle("alice", &test_domains()).is_ok());
        assert!(validate_handle("alice123", &test_domains()).is_ok());
        assert!(validate_handle("alice-bob", &test_domains()).is_ok());
        assert!(validate_handle("alice.bsky.social", &test_domains()).is_ok());
        assert!(validate_handle("test.example.com", &test_domains()).is_ok());
    }

    #[test]
    fn test_invalid_handles() {
        // Too short
        assert!(validate_handle("ab", &test_domains()).is_err());

        // Too long
        let long = "a".repeat(254);
        assert!(validate_handle(&long, &test_domains()).is_err());

        // Invalid characters
        assert!(validate_handle("alice@bob", &test_domains()).is_err());
        assert!(validate_handle("alice bob", &test_domains()).is_err());
        assert!(validate_handle("alice_bob", &test_domains()).is_err());

        // Leading/trailing hyphens
        assert!(validate_handle("-alice", &test_domains()).is_err());
        assert!(validate_handle("alice-", &test_domains()).is_err());

        // Consecutive dots
        assert!(validate_handle("alice..bob", &test_domains()).is_err());

        // Uppercase
        assert!(validate_handle("Alice", &test_domains()).is_err());
        assert!(validate_handle("ALICE", &test_domains()).is_err());
    }

    #[test]
    fn test_tld_validation() {
        assert!(is_valid_tld("com"));
        assert!(is_valid_tld("net"));
        assert!(is_valid_tld("org"));
        assert!(is_valid_tld("io"));
        assert!(!is_valid_tld("invalid"));
        assert!(!is_valid_tld("xyz123"));
    }

    #[test]
    fn test_slur_detection() {
        assert!(has_explicit_slur("admin"));
        assert!(has_explicit_slur("system-user"));
        assert!(!has_explicit_slur("alice"));
        assert!(!has_explicit_slur("bob123"));
    }

    #[test]
    fn test_normalization() {
        assert_eq!(normalize_handle("Alice"), "alice");
        assert_eq!(normalize_handle("ALICE"), "alice");
        assert_eq!(normalize_handle("AlIcE"), "alice");
        assert_eq!(normalize_handle("alice"), "alice");
    }

    #[test]
    fn test_label_length() {
        // Valid label length
        let handle = format!("{}.com", "a".repeat(63));
        assert!(validate_handle(&handle, &test_domains()).is_ok());

        // Invalid label length (>63)
        let handle = format!("{}.com", "a".repeat(64));
        assert!(validate_handle(&handle, &test_domains()).is_err());
    }

    #[test]
    fn test_service_domains() {
        // Service domain should be allowed even without standard TLD
        assert!(validate_handle("alice.bsky.social", &test_domains()).is_ok());
        assert!(validate_handle("bob.localhost", &test_domains()).is_ok());
    }
}
