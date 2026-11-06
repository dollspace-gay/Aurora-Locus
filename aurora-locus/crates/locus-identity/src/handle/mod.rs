//! Handle rules: normalization and syntax, the extra rules for handles on
//! this PDS's own handle domains, reserved names and the slur filter, as the
//! reference PDS applies them.

mod reserved;
mod slurs;

use std::sync::LazyLock;

use regex::Regex;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HandleError {
    /// Not handle syntax at all (what a lexicon `handle` format refuses).
    #[error("{0}")]
    Syntax(String),
    /// A disallowed TLD or a service-domain rule (atproto `InvalidHandle`).
    #[error("{0}")]
    Invalid(String),
    /// Reserved or otherwise not available (atproto `HandleNotAvailable`).
    #[error("{0}")]
    NotAvailable(String),
}

/// Lowercases and checks atproto handle syntax, refusing disallowed TLDs.
/// Surrounding whitespace is a syntax error, as in the reference.
pub fn normalize_and_validate(handle: &str) -> Result<String, HandleError> {
    let handle = handle.to_ascii_lowercase();
    if proto_blue_syntax::Handle::new(&handle).is_err() {
        return Err(HandleError::Syntax("Invalid handle".into()));
    }
    let tld = handle.rsplit('.').next().unwrap_or_default();
    if DISALLOWED_TLDS.contains(&tld) {
        return Err(HandleError::Invalid(
            "Handle TLD is invalid or disallowed".into(),
        ));
    }
    Ok(handle)
}

/// TLDs atproto handles may not use (the reference's syntax rules).
const DISALLOWED_TLDS: &[&str] = &[
    "alt",
    "arpa",
    "example",
    "internal",
    "invalid",
    "local",
    "localhost",
    "onion",
];

/// Limits on the first label of a handle on one of this PDS's handle domains.
#[derive(Debug, Clone, Copy)]
pub struct LabelLimits {
    pub min: usize,
    pub max: usize,
}

impl Default for LabelLimits {
    /// The reference's limits: 3 to 18 characters.
    fn default() -> Self {
        Self { min: 3, max: 18 }
    }
}

/// The handle domain (like `.example.com`) a handle ends with, if any.
#[must_use]
pub fn service_domain<'a>(handle: &str, domains: &'a [String]) -> Option<&'a str> {
    domains
        .iter()
        .map(String::as_str)
        .find(|d| handle.ends_with(d))
}

/// The extra rules for a handle on a handle domain: its first label has no
/// dots, fits `limits`, and isn't reserved (unless `allow_reserved`, for
/// operators).
pub fn ensure_service_constraints(
    handle: &str,
    domains: &[String],
    limits: LabelLimits,
    extra_reserved: &[String],
    allow_reserved: bool,
) -> Result<(), HandleError> {
    let domain = service_domain(handle, domains).unwrap_or("");
    let front = &handle[..handle.len() - domain.len()];
    if front.contains('.') {
        return Err(HandleError::Invalid("Invalid characters in handle".into()));
    }
    if front.len() < limits.min {
        return Err(HandleError::Invalid("Handle too short".into()));
    }
    if front.len() > limits.max {
        return Err(HandleError::Invalid("Handle too long".into()));
    }
    if !allow_reserved
        && (is_reserved(front) || extra_reserved.iter().any(|r| r.eq_ignore_ascii_case(front)))
    {
        return Err(HandleError::NotAvailable("Reserved handle".into()));
    }
    Ok(())
}

/// Whether a first label is on the reference's reserved list.
#[must_use]
pub fn is_reserved(label: &str) -> bool {
    reserved::RESERVED.binary_search(&label).is_ok()
}

static SLURS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    slurs::PATTERNS
        .iter()
        // JavaScript's `\b` (without the `u` flag) is an ASCII word boundary.
        .map(|p| Regex::new(&p.replace(r"\b", r"(?-u:\b)")).expect("slur patterns compile"))
        .collect()
});

/// The reference's slur filter: the handle, and the handle with `.`, `-`
/// and `_` removed, are checked against each pattern.
#[must_use]
pub fn has_explicit_slur(handle: &str) -> bool {
    let squashed: String = handle
        .chars()
        .filter(|c| !matches!(c, '.' | '-' | '_'))
        .collect();
    SLURS
        .iter()
        .any(|re| re.is_match(handle) || re.is_match(&squashed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domains() -> Vec<String> {
        vec![".example.com".into()]
    }

    #[test]
    fn normalizes_and_validates() {
        assert_eq!(
            normalize_and_validate("Alice.Example.COM").unwrap(),
            "alice.example.com"
        );
        assert!(normalize_and_validate("no_underscores.example.com").is_err());
        assert!(normalize_and_validate("single").is_err());
        assert!(normalize_and_validate("alice.local").is_err());
        assert!(normalize_and_validate("alice.onion").is_err());
    }

    /// The non-comment lines of an interop syntax file, untrimmed.
    fn vectors(file: &str) -> Vec<String> {
        let path = format!(
            "{}/../../interop-test-files/syntax/{file}",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{path}: {e}"))
            .split('\n')
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(str::to_owned)
            .collect()
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig { cases: 2048, ..Default::default() })]

        /// Arbitrary and handle-like strings: every rule answers, none
        /// panics, and what's accepted is lowercase and accepted again.
        #[test]
        fn handle_rules_never_panic(
            s in proptest::prop_oneof![
                ".{0,40}",
                "[a-zA-Z0-9.\\-_ ]{0,30}(\\.example\\.com)?",
                "[a-z]{1,20}\\.[a-z]{2,6}",
            ],
            min in 0usize..6,
            max in 0usize..30,
        ) {
            if let Ok(n) = normalize_and_validate(&s) {
                proptest::prop_assert_eq!(&n, &n.to_ascii_lowercase());
                proptest::prop_assert_eq!(normalize_and_validate(&n), Ok(n.clone()));
            }
            let _ = ensure_service_constraints(&s, &domains(), LabelLimits { min, max }, &[s.clone()], false);
            let _ = has_explicit_slur(&s);
            let _ = is_reserved(&s);
        }
    }

    #[test]
    fn interop_handle_vectors() {
        let valid = vectors("handle_syntax_valid.txt");
        assert!(valid.len() > 40);
        for h in &valid {
            // Valid syntax normalizes to lowercase; only a disallowed TLD
            // (a separate rule) refuses it.
            let lower = h.to_ascii_lowercase();
            match normalize_and_validate(h) {
                Ok(n) => assert_eq!(n, lower, "{h:?}"),
                Err(HandleError::Invalid(_)) => assert!(
                    DISALLOWED_TLDS.contains(&lower.rsplit('.').next().unwrap()),
                    "{h:?}"
                ),
                Err(e) => panic!("{h:?} refused: {e}"),
            }
        }
        let invalid = vectors("handle_syntax_invalid.txt");
        assert!(invalid.len() > 20);
        for h in &invalid {
            assert!(
                matches!(normalize_and_validate(h), Err(HandleError::Syntax(_))),
                "{h:?} accepted"
            );
        }
    }

    #[test]
    fn service_domain_rules_match_the_reference() {
        let lim = LabelLimits::default();
        assert!(
            ensure_service_constraints("alice.example.com", &domains(), lim, &[], false).is_ok()
        );
        assert_eq!(
            ensure_service_constraints("al.example.com", &domains(), lim, &[], false),
            Err(HandleError::Invalid("Handle too short".into()))
        );
        assert_eq!(
            ensure_service_constraints(
                "a234567890123456789.example.com",
                &domains(),
                lim,
                &[],
                false
            ),
            Err(HandleError::Invalid("Handle too long".into()))
        );
        assert_eq!(
            ensure_service_constraints("a.b.example.com", &domains(), lim, &[], false),
            Err(HandleError::Invalid("Invalid characters in handle".into()))
        );
        assert_eq!(
            ensure_service_constraints("admin.example.com", &domains(), lim, &[], false),
            Err(HandleError::NotAvailable("Reserved handle".into()))
        );
        assert!(
            ensure_service_constraints("admin.example.com", &domains(), lim, &[], true).is_ok()
        );
        // Operator-entered names (like the domain's existing subdomains).
        let extra = vec!["mail".to_owned()];
        assert_eq!(
            ensure_service_constraints("mail.example.com", &domains(), lim, &extra, false),
            Err(HandleError::NotAvailable("Reserved handle".into()))
        );
    }

    #[test]
    fn reserved_list_is_vendored_and_sorted() {
        assert!(reserved::RESERVED.len() > 1000);
        assert!(reserved::RESERVED.windows(2).all(|w| w[0] < w[1]));
        for name in ["pds", "plc", "admin", "support"] {
            assert!(is_reserved(name), "{name}");
        }
        assert!(!is_reserved("alice"));
    }

    #[test]
    fn slur_filter_compiles_and_matches_like_the_reference() {
        assert_eq!(SLURS.len(), 7);
        // Ordinary handles pass, including ones containing similar letters.
        for ok in [
            "alice.example.com",
            "chinchilla.example.com",
            "spicy.example.com",
            "skunk.example.com",
        ] {
            assert!(!has_explicit_slur(ok), "{ok}");
        }
        // The filter catches a listed word. Assembled from pieces to keep the
        // word out of the source. (The reference also strips `.`, `-` and `_`
        // from the whole handle, domain included, so that pass only helps
        // patterns without a trailing word boundary; this word's has one.)
        let word = ["ch", "in", "k"].concat();
        assert!(has_explicit_slur(&format!("{word}.example.com")));
        // Inside a longer word it doesn't match (word boundaries).
        assert!(!has_explicit_slur(&format!("x{word}x.example.com")));
    }
}
