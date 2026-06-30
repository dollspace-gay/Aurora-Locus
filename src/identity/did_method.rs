//! DID-method classifier — the single source of truth for "what method is this DID?"
//!
//! Replaces ad-hoc `did.starts_with("did:plc:")` / `did.starts_with("did:web:")`
//! checks scattered across the codebase. Phase 0 (v0.10 Arc 1, chainlink #414)
//! introduces the module and uses it in the bulk-update method filter
//! (`run_bulk_diddoc_update`); Phase A migrates the remaining ad-hoc sites, and
//! Phase D's per-account serve route consumes `domain` + `segment`.

use thiserror::Error;

/// The DID methods Aurora-Locus hosts or resolves. `Plc` is the external
/// PLC-directory-anchored method; `Web` is the locally-hosted did:web method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DidMethod {
    Plc,
    Web,
}

/// A classified DID — its method plus the method-specific parts the serve route
/// and bulk filter need. For `Plc`, `domain`/`segment` are `None`. For `Web`,
/// `domain` is the host and `segment` is the path-form remainder (colon-joined
/// segments after the host, re-joined with `/` to match the resolver's URL
/// mapping — e.g. `did:web:example.com:user:alice` → `domain=example.com`,
/// `segment=user/alice`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDid {
    pub method: DidMethod,
    pub domain: Option<String>,
    pub segment: Option<String>,
    pub raw: String,
}

impl ParsedDid {
    /// The classified method. Convenience accessor for guard sites that only need
    /// to discriminate `Plc` vs `Web` and don't care about the parts.
    pub fn method(&self) -> DidMethod {
        self.method
    }
}

/// Why a DID string could not be classified. A well-formed `actor.did` always
/// parses; these are returned for malformed or unsupported input.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseDidError {
    #[error("empty DID string")]
    Empty,
    #[error("DID missing 'did:' prefix")]
    MissingPrefix,
    #[error("unsupported DID method: {0}")]
    UnsupportedMethod(String),
    #[error("malformed did:plc: missing suffix")]
    MalformedPlc,
    #[error("malformed did:web: missing host")]
    MalformedWeb,
}

/// Classify a DID string into its method and method-specific parts.
///
/// Does not trim or normalize: leading whitespace makes the `did:` prefix absent
/// (rejected); interior content is taken verbatim. This is deliberate — DIDs are
/// canonical identifiers and silent normalization would mask data problems.
pub fn parse_did(did: &str) -> Result<ParsedDid, ParseDidError> {
    if did.is_empty() {
        return Err(ParseDidError::Empty);
    }
    let rest = did.strip_prefix("did:").ok_or(ParseDidError::MissingPrefix)?;

    // Split on the first ':' after "did:" to separate method from the
    // method-specific identifier.
    let (method_str, identifier) = rest
        .split_once(':')
        .ok_or_else(|| ParseDidError::UnsupportedMethod(rest.to_string()))?;

    match method_str {
        "plc" => {
            if identifier.is_empty() {
                return Err(ParseDidError::MalformedPlc);
            }
            Ok(ParsedDid {
                method: DidMethod::Plc,
                domain: None,
                segment: None,
                raw: did.to_string(),
            })
        }
        "web" => {
            // did:web:host  OR  did:web:host:path:segments
            // Segments after the host are re-joined with '/' into the path-form
            // segment (mirrors IdentityResolver::fetch_web_document's URL mapping).
            let mut parts = identifier.split(':');
            let host = parts.next().ok_or(ParseDidError::MalformedWeb)?;
            if host.is_empty() {
                return Err(ParseDidError::MalformedWeb);
            }
            let remaining: Vec<&str> = parts.collect();
            let segment = if remaining.is_empty() {
                None
            } else {
                Some(remaining.join("/"))
            };
            Ok(ParsedDid {
                method: DidMethod::Web,
                domain: Some(host.to_string()),
                segment,
                raw: did.to_string(),
            })
        }
        other => Err(ParseDidError::UnsupportedMethod(other.to_string())),
    }
}

/// True iff `did` is a well-formed did:plc. Ergonomic guard for the many sites
/// that only need to admit/reject the PLC method. A malformed or non-plc DID is
/// `false`, so `!is_plc(did)` rejects did:web, did:key, and malformed input —
/// exactly matching the prior `!did.starts_with("did:plc:")` guards.
pub fn is_plc(did: &str) -> bool {
    matches!(parse_did(did), Ok(p) if p.method == DidMethod::Plc)
}

/// True iff `did` is a well-formed did:web. Mirror of [`is_plc`].
pub fn is_web(did: &str) -> bool {
    matches!(parse_did(did), Ok(p) if p.method == DidMethod::Web)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plc_did() {
        let p = parse_did("did:plc:abcdef123456").unwrap();
        assert_eq!(p.method(), DidMethod::Plc);
        assert!(p.domain.is_none());
        assert!(p.segment.is_none());
        assert_eq!(p.raw, "did:plc:abcdef123456");
    }

    #[test]
    fn parses_web_did_host_only() {
        let p = parse_did("did:web:example.com").unwrap();
        assert_eq!(p.method(), DidMethod::Web);
        assert_eq!(p.domain.as_deref(), Some("example.com"));
        assert!(p.segment.is_none());
    }

    #[test]
    fn parses_web_did_path_form() {
        let p = parse_did("did:web:example.com:user:alice").unwrap();
        assert_eq!(p.method(), DidMethod::Web);
        assert_eq!(p.domain.as_deref(), Some("example.com"));
        assert_eq!(p.segment.as_deref(), Some("user/alice"));
    }

    #[test]
    fn parses_web_did_deep_path() {
        let p = parse_did("did:web:example.com:a:b:c:d").unwrap();
        assert_eq!(p.domain.as_deref(), Some("example.com"));
        assert_eq!(p.segment.as_deref(), Some("a/b/c/d"));
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(parse_did(""), Err(ParseDidError::Empty));
    }

    #[test]
    fn rejects_missing_prefix() {
        assert!(matches!(
            parse_did("plc:abc"),
            Err(ParseDidError::MissingPrefix)
        ));
        assert!(matches!(
            parse_did("example.com"),
            Err(ParseDidError::MissingPrefix)
        ));
    }

    #[test]
    fn rejects_unsupported_method() {
        assert!(matches!(
            parse_did("did:key:zABC"),
            Err(ParseDidError::UnsupportedMethod(_))
        ));
        assert!(matches!(
            parse_did("did:ion:abc"),
            Err(ParseDidError::UnsupportedMethod(_))
        ));
    }

    #[test]
    fn rejects_malformed_plc() {
        assert_eq!(parse_did("did:plc:"), Err(ParseDidError::MalformedPlc));
    }

    #[test]
    fn rejects_malformed_web() {
        assert_eq!(parse_did("did:web:"), Err(ParseDidError::MalformedWeb));
    }

    #[test]
    fn is_plc_is_web_helpers() {
        assert!(is_plc("did:plc:abc"));
        assert!(!is_plc("did:web:example.com"));
        assert!(!is_plc("did:key:zABC"));
        assert!(!is_plc("did:plc:")); // malformed → false (so !is_plc rejects it)
        assert!(!is_plc("garbage"));
        assert!(is_web("did:web:example.com:user:alice"));
        assert!(!is_web("did:plc:abc"));
        assert!(!is_web("did:web:")); // malformed → false
    }

    #[test]
    fn does_not_trim_or_normalize() {
        // Leading whitespace means no "did:" prefix → rejected.
        assert!(matches!(
            parse_did("  did:plc:abc"),
            Err(ParseDidError::MissingPrefix)
        ));
        // Interior/trailing content is taken verbatim (a trailing space is a
        // non-empty plc suffix); classification succeeds, raw is unmodified.
        let p = parse_did("did:plc:abc ").unwrap();
        assert_eq!(p.method(), DidMethod::Plc);
        assert_eq!(p.raw, "did:plc:abc ");
    }
}
