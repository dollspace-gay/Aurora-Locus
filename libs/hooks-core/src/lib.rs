//! hooks-core — structurally-firewalled declaration logic for Aurora-Locus
//! integration hooks (v0.9 Phase A, #350; design addendum §3).
//!
//! **Layer 1 tripwire (structural):** this crate's `Cargo.toml` declares NO
//! HTTP-client-exposing dependency, direct or transitive. The build system
//! refuses to compile any networking call here because no networking crate is
//! in the dependency closure — verify any time with `cargo tree --package
//! hooks-core`. This is the "declaration without execution" guarantee for the
//! high-leverage core: the declaration types + validation can never become an
//! execution sink, enforced by the dependency graph, not a maintained check.
//!
//! Pure logic only — no I/O, no DB, no atproto types. The wired surface (XRPC
//! handlers, DB, audit, UI) lives in the `aurora-locus` crate, which calls in.

pub mod netaddr;

use serde::Serialize;
use url::{Host, Url};

/// Maximum stored URL length (design-commit 2).
pub const MAX_URL_LEN: usize = 2048;
/// Maximum description length (design-commit 10).
pub const MAX_DESCRIPTION_LEN: usize = 4096;
/// Active-hook cap (design-commit 11).
pub const MAX_ACTIVE_HOOKS: i64 = 50;

/// The v0.9 closed-set event-class taxonomy (§2.2 / design-commit 14). The
/// AVAILABLE subset (post drop-policy) is computed by the wired crate from
/// substrate-emission survival; validation takes the available set as input.
pub const V0_9_EVENT_CLASSES: &[&str] = &[
    "moderation.report-submitted",
    "moderation.report-resolved",
    "moderation.escalation-triggered",
    "moderation.account-suspended",
    "moderation.account-restored",
    "moderation.account-labeled",
    "account.created",
    "system.tier-changed",
];

/// One integration-hook declaration (§2.1 / §3.1). Wire shape for `listHooks`
/// / composite-load; the DB layer in `aurora-locus` maps rows to this.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hook {
    pub id: String,
    pub name: String,
    pub url: String,
    pub event_classes: Vec<String>,
    pub description: Option<String>,
    pub enabled: bool,
    pub created_at: String,
    pub created_by_did: String,
    pub last_modified_at: String,
    pub last_modified_by_did: String,
    pub rationale: Option<String>,
    pub deleted_at: Option<String>,
}

/// Config-time validation failures. `code` is the stable wire error code; the
/// `Display` message is operator-facing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    UrlTooLong,
    UrlParse,
    SchemeNotAllowed,
    UserinfoNotAllowed,
    HostRequired,
    InvalidHost,
    IpNotAllowed,
    NameRequired,
    NameTooLong,
    DescriptionTooLong,
    EmptyEventClasses,
    UnknownEventClass(String),
}

impl ValidationError {
    pub fn code(&self) -> &'static str {
        match self {
            ValidationError::UrlTooLong => "url-too-long",
            ValidationError::UrlParse => "url-parse-failed",
            ValidationError::SchemeNotAllowed => "scheme-not-allowed",
            ValidationError::UserinfoNotAllowed => "userinfo-not-allowed",
            ValidationError::HostRequired => "host-required",
            ValidationError::InvalidHost => "invalid-host",
            ValidationError::IpNotAllowed => "ip-not-allowed",
            ValidationError::NameRequired => "name-required",
            ValidationError::NameTooLong => "name-too-long",
            ValidationError::DescriptionTooLong => "description-too-long",
            ValidationError::EmptyEventClasses => "empty-event-classes",
            ValidationError::UnknownEventClass(_) => "unknown-or-dropped-event-class",
        }
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::UrlTooLong => write!(f, "URL exceeds {} characters", MAX_URL_LEN),
            ValidationError::UrlParse => write!(f, "URL could not be parsed"),
            ValidationError::SchemeNotAllowed => write!(f, "only https URLs are allowed"),
            ValidationError::UserinfoNotAllowed => write!(f, "URL must not contain userinfo (user:password@)"),
            ValidationError::HostRequired => write!(f, "URL must have a host"),
            ValidationError::InvalidHost => write!(f, "URL host is invalid"),
            ValidationError::IpNotAllowed => write!(f, "URL host resolves to a disallowed IP range"),
            ValidationError::NameRequired => write!(f, "name is required"),
            ValidationError::NameTooLong => write!(f, "name is too long"),
            ValidationError::DescriptionTooLong => write!(f, "description exceeds {} characters", MAX_DESCRIPTION_LEN),
            ValidationError::EmptyEventClasses => write!(f, "at least one event class is required"),
            ValidationError::UnknownEventClass(c) => write!(f, "unknown or unavailable event class: {}", c),
        }
    }
}

impl std::error::Error for ValidationError {}

/// The 9-step URL validation + normalization pipeline (§2.4 / design-commits
/// 1-9 + 31-35). Returns the stored (normalized, fragment-stripped, WHATWG-
/// serialized) form. No DNS resolution (design-commit 1) — IP literals are
/// range-checked; hostnames are accepted as-is (resolution-time SSRF is the
/// future execution cycle's concern).
pub fn validate_hook_url(input: &str) -> Result<String, ValidationError> {
    // Step 0: length bound.
    if input.len() > MAX_URL_LEN {
        return Err(ValidationError::UrlTooLong);
    }
    // Step 1: parse.
    let mut url = Url::parse(input).map_err(|_| ValidationError::UrlParse)?;
    // Step 2: scheme allowlist (https only).
    if url.scheme() != "https" {
        return Err(ValidationError::SchemeNotAllowed);
    }
    // Step 3: userinfo rejection.
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ValidationError::UserinfoNotAllowed);
    }
    // Step 4: host extraction.
    let host = url.host().ok_or(ValidationError::HostRequired)?;
    // Step 5: dot-only host rejection.
    match &host {
        Host::Domain(d) if d.is_empty() || *d == "." || *d == ".." => {
            return Err(ValidationError::InvalidHost);
        }
        _ => {}
    }
    // Step 6: IP literal range checks.
    match &host {
        Host::Ipv4(addr) if netaddr::reject_ipv4(addr) => return Err(ValidationError::IpNotAllowed),
        Host::Ipv6(addr) if netaddr::reject_ipv6(addr) => return Err(ValidationError::IpNotAllowed),
        _ => {}
    }
    // Step 7: any port allowed (no restriction at config time).
    // Step 8: arbitrary path/query allowed.
    // Step 9: strip fragment + return the WHATWG-normalized serialization.
    url.set_fragment(None);
    Ok(url.as_str().to_string())
}

/// Validate the operator-supplied name + description bounds (§2.1).
pub fn validate_name(name: &str) -> Result<(), ValidationError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ValidationError::NameRequired);
    }
    if name.chars().count() > 256 {
        return Err(ValidationError::NameTooLong);
    }
    Ok(())
}

/// Validate the description bound (§2.1 / design-commit 10).
pub fn validate_description(description: Option<&str>) -> Result<(), ValidationError> {
    if let Some(d) = description {
        if d.chars().count() > MAX_DESCRIPTION_LEN {
            return Err(ValidationError::DescriptionTooLong);
        }
    }
    Ok(())
}

/// Validate the event-class subscription against the AVAILABLE closed set
/// (§2.2 / design-commits 14 + 16). `available` is the post-drop-policy subset
/// the wired crate supplies (substrate as single source of truth).
pub fn validate_event_classes(classes: &[String], available: &[&str]) -> Result<(), ValidationError> {
    if classes.is_empty() {
        return Err(ValidationError::EmptyEventClasses);
    }
    for c in classes {
        if !available.contains(&c.as_str()) {
            return Err(ValidationError::UnknownEventClass(c.clone()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_https_url_and_strips_fragment() {
        let out = validate_hook_url("https://example.com/webhook?x=1#frag").unwrap();
        assert_eq!(out, "https://example.com/webhook?x=1");
    }

    #[test]
    fn rejects_non_https() {
        assert_eq!(validate_hook_url("http://example.com").unwrap_err(), ValidationError::SchemeNotAllowed);
    }

    #[test]
    fn rejects_userinfo() {
        assert_eq!(
            validate_hook_url("https://user:pass@example.com").unwrap_err(),
            ValidationError::UserinfoNotAllowed
        );
    }

    #[test]
    fn rejects_too_long() {
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_LEN));
        assert_eq!(validate_hook_url(&long).unwrap_err(), ValidationError::UrlTooLong);
    }

    #[test]
    fn rejects_internal_ip_hosts() {
        for u in ["https://127.0.0.1/x", "https://10.0.0.1/x", "https://[::1]/x", "https://[fc00::1]/x"] {
            assert_eq!(validate_hook_url(u).unwrap_err(), ValidationError::IpNotAllowed, "{}", u);
        }
    }

    #[test]
    fn host_case_folds_on_normalize() {
        let out = validate_hook_url("https://Example.COM/x").unwrap();
        assert!(out.starts_with("https://example.com/"));
    }

    #[test]
    fn event_classes_closed_set() {
        let avail: Vec<&str> = V0_9_EVENT_CLASSES.to_vec();
        assert!(validate_event_classes(&["account.created".into()], &avail).is_ok());
        assert!(matches!(
            validate_event_classes(&["bogus.class".into()], &avail).unwrap_err(),
            ValidationError::UnknownEventClass(_)
        ));
        assert_eq!(validate_event_classes(&[], &avail).unwrap_err(), ValidationError::EmptyEventClasses);
    }
}
