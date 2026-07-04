
//! Reserved handle system - prevents registration of system-level and protocol handles
//!
//! This module maintains a list of reserved handles that cannot be registered by users.
//! These include system names, protocol names, and common administrative handles.

use lazy_static::lazy_static;
use std::collections::HashSet;

lazy_static! {
    /// Static set of reserved handles
    static ref RESERVED_HANDLES: HashSet<&'static str> = {
        let mut set = HashSet::new();

    // System and administrative handles
    set.insert("admin");
    set.insert("administrator");
    set.insert("root");
    set.insert("system");
    set.insert("moderator");
    set.insert("mod");

    // Network and infrastructure
    set.insert("www");
    set.insert("api");
    set.insert("cdn");
    set.insert("mail");
    set.insert("email");
    set.insert("smtp");
    set.insert("imap");
    set.insert("pop");
    set.insert("pop3");
    set.insert("ftp");
    set.insert("sftp");
    set.insert("ssh");
    set.insert("vpn");
    set.insert("ns");
    set.insert("ns1");
    set.insert("ns2");
    set.insert("dns");
    set.insert("mx");
    set.insert("mx1");
    set.insert("mx2");

    // Web and application
    set.insert("blog");
    set.insert("forum");
    set.insert("shop");
    set.insert("store");
    set.insert("news");
    set.insert("media");
    set.insert("static");
    set.insert("assets");
    set.insert("files");
    set.insert("images");
    set.insert("img");
    set.insert("css");
    set.insert("js");
    set.insert("downloads");
    set.insert("upload");
    set.insert("uploads");

    // Authentication and security
    set.insert("auth");
    set.insert("login");
    set.insert("signin");
    set.insert("signup");
    set.insert("register");
    set.insert("logout");
    set.insert("signout");
    set.insert("password");
    set.insert("reset");
    set.insert("verify");
    set.insert("oauth");
    set.insert("sso");
    set.insert("saml");

    // Account management
    set.insert("account");
    set.insert("accounts");
    set.insert("user");
    set.insert("users");
    set.insert("profile");
    set.insert("profiles");
    set.insert("settings");
    set.insert("preferences");
    set.insert("prefs");

    // Support and help
    set.insert("help");
    set.insert("support");
    set.insert("contact");
    set.insert("feedback");
    set.insert("abuse");
    set.insert("report");
    set.insert("dmca");

    // Legal and documentation
    set.insert("about");
    set.insert("terms");
    set.insert("tos");
    set.insert("privacy");
    set.insert("policy");
    set.insert("legal");
    set.insert("copyright");
    set.insert("dmca");
    set.insert("docs");
    set.insert("documentation");
    set.insert("faq");

    // ATProto and Bluesky specific
    set.insert("atproto");
    set.insert("at");
    set.insert("did");
    set.insert("plc");
    set.insert("xrpc");
    set.insert("bsky");
    set.insert("bluesky");
    set.insert("lexicon");
    set.insert("nsid");

    // Protocol and standards
    set.insert("http");
    set.insert("https");
    set.insert("ws");
    set.insert("wss");
    set.insert("websocket");
    set.insert("ipfs");
    set.insert("ipns");

    // Development and testing
    set.insert("test");
    set.insert("testing");
    set.insert("dev");
    set.insert("development");
    set.insert("staging");
    set.insert("prod");
    set.insert("production");
    set.insert("demo");
    set.insert("example");
    set.insert("sandbox");

    // Status and monitoring
    set.insert("status");
    set.insert("health");
    set.insert("ping");
    set.insert("metrics");
    set.insert("stats");
    set.insert("analytics");
    set.insert("monitoring");

    // Security and safety
    set.insert("security");
    set.insert("safety");
    set.insert("trust");
    set.insert("verify");
    set.insert("verified");
    set.insert("official");

    // Generic service names
    set.insert("service");
    set.insert("services");
    set.insert("app");
    set.insert("apps");
    set.insert("web");
    set.insert("mobile");
    set.insert("desktop");

    // Reserved for future use
    set.insert("aurora");
    set.insert("locus");
    set.insert("pds");
    set.insert("relay");
    set.insert("appview");
    set.insert("feed");
    set.insert("feeds");

        set
    };
}

/// Check if a handle is reserved
pub fn is_reserved(handle: &str) -> bool {
    let normalized = handle.to_lowercase();
    RESERVED_HANDLES.contains(normalized.as_str())
}

/// Check if a handle is reserved, returning a detailed reason
#[allow(dead_code)] // exercised by in-file tests only; no production consumer
pub fn check_reserved(handle: &str) -> Result<(), String> {
    if is_reserved(handle) {
        Err(format!(
            "Handle '{}' is reserved and cannot be registered. Please choose a different handle.",
            handle
        ))
    } else {
        Ok(())
    }
}

/// Get the full list of reserved handles (for administrative purposes)
#[allow(dead_code)] // exercised by in-file tests only; no production consumer
pub fn get_reserved_handles() -> Vec<&'static str> {
    RESERVED_HANDLES.iter().copied().collect()
}

/// Get count of reserved handles
#[allow(dead_code)] // exercised by in-file tests only; no production consumer
pub fn reserved_count() -> usize {
    RESERVED_HANDLES.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reserved_system_handles() {
        assert!(is_reserved("admin"));
        assert!(is_reserved("root"));
        assert!(is_reserved("system"));
        assert!(is_reserved("moderator"));
    }

    #[test]
    fn test_reserved_network_handles() {
        assert!(is_reserved("www"));
        assert!(is_reserved("api"));
        assert!(is_reserved("cdn"));
        assert!(is_reserved("mail"));
        assert!(is_reserved("dns"));
    }

    #[test]
    fn test_reserved_auth_handles() {
        assert!(is_reserved("login"));
        assert!(is_reserved("signup"));
        assert!(is_reserved("oauth"));
        assert!(is_reserved("auth"));
    }

    #[test]
    fn test_reserved_atproto_handles() {
        assert!(is_reserved("atproto"));
        assert!(is_reserved("did"));
        assert!(is_reserved("plc"));
        assert!(is_reserved("xrpc"));
        assert!(is_reserved("bsky"));
    }

    #[test]
    fn test_case_insensitive() {
        assert!(is_reserved("ADMIN"));
        assert!(is_reserved("Admin"));
        assert!(is_reserved("aDmIn"));
        assert!(is_reserved("admin"));
    }

    #[test]
    fn test_non_reserved_handles() {
        assert!(!is_reserved("alice"));
        assert!(!is_reserved("bob"));
        assert!(!is_reserved("charlie"));
        assert!(!is_reserved("myhandle"));
        assert!(!is_reserved("user123"));
    }

    #[test]
    fn test_check_reserved_ok() {
        assert!(check_reserved("alice").is_ok());
        assert!(check_reserved("myhandle").is_ok());
    }

    #[test]
    fn test_check_reserved_err() {
        let result = check_reserved("admin");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("reserved"));
    }

    #[test]
    fn test_reserved_count() {
        let count = reserved_count();
        assert!(
            count > 100,
            "Should have substantial list of reserved handles"
        );
    }

    #[test]
    fn test_get_reserved_handles() {
        let handles = get_reserved_handles();
        assert!(handles.contains(&"admin"));
        assert!(handles.contains(&"api"));
        assert!(handles.contains(&"atproto"));
    }

    #[test]
    fn test_subdomain_like_handles() {
        // These should NOT be reserved as they're valid user handles
        assert!(!is_reserved("admin-user"));
        assert!(!is_reserved("my-admin"));
        assert!(!is_reserved("www1"));
        assert!(!is_reserved("api-client"));
    }
}
