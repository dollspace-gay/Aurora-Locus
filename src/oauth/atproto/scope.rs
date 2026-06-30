//! atproto-OAuth scope vocabulary (Arc 2 Phase β.3, chainlink #420 /
//! LOCKED design §3.2 / R1 F-3.5).
//!
//! atproto OAuth uses its own scope grammar — bare tokens like `atproto` and
//! the `transition:*` migration scopes — distinct from Aurora's internal
//! colon-namespaced capability scopes in [`crate::oauth::scope`]. This module
//! is a **parallel** vocabulary, *not* a translation layer: bearers issued by
//! the atproto provider carry these atproto-spec scopes verbatim, and any
//! capability check against an atproto bearer is made against this vocabulary
//! at handler entry. If a future cycle wants to project atproto scopes onto
//! the internal capability model, that projection ships then.
//!
//! The set is deliberately closed: an authorize/PAR request naming a scope
//! outside this enum is rejected, so the provider never issues a bearer
//! carrying a scope it cannot reason about.

use std::fmt;
use std::str::FromStr;

/// The error returned when a scope token is not a recognised atproto scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownScope(pub String);

impl fmt::Display for UnknownScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown atproto scope: {}", self.0)
    }
}

impl std::error::Error for UnknownScope {}

/// A single atproto-OAuth scope.
///
/// `atproto` is the base scope (required by the spec for any session);
/// `transition:generic` and `transition:chat.bsky` are the migration scopes
/// that grant a client the legacy app-password-equivalent surface during the
/// OAuth transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AtprotoScope {
    /// `atproto` — the base scope; presence is required for a usable session.
    Atproto,
    /// `transition:generic` — generic legacy-equivalent access.
    TransitionGeneric,
    /// `transition:chat.bsky` — legacy chat (DM) access.
    TransitionChatBsky,
}

impl AtprotoScope {
    /// The canonical bare-token spelling of this scope.
    pub fn as_str(&self) -> &'static str {
        match self {
            AtprotoScope::Atproto => "atproto",
            AtprotoScope::TransitionGeneric => "transition:generic",
            AtprotoScope::TransitionChatBsky => "transition:chat.bsky",
        }
    }

    /// Every scope the provider advertises in its AS metadata
    /// (`scopes_supported`), in canonical order.
    pub fn all() -> [AtprotoScope; 3] {
        [
            AtprotoScope::Atproto,
            AtprotoScope::TransitionGeneric,
            AtprotoScope::TransitionChatBsky,
        ]
    }

    /// Parse a space-separated scope string into the ordered, de-duplicated
    /// set it denotes.
    ///
    /// Rules (atproto-OAuth + RFC 6749 §3.3): tokens are space-separated;
    /// empty input is rejected (a request must name at least one scope); the
    /// base `atproto` scope MUST be present; any unrecognised token rejects
    /// the whole string (closed vocabulary). Duplicate tokens collapse.
    pub fn parse_set(s: &str) -> Result<ScopeSet, ScopeParseError> {
        let mut scopes: Vec<AtprotoScope> = Vec::new();
        for token in s.split_whitespace() {
            let scope = AtprotoScope::from_str(token)
                .map_err(|e| ScopeParseError::Unknown(e.0))?;
            if !scopes.contains(&scope) {
                scopes.push(scope);
            }
        }
        if scopes.is_empty() {
            return Err(ScopeParseError::Empty);
        }
        if !scopes.contains(&AtprotoScope::Atproto) {
            return Err(ScopeParseError::MissingBase);
        }
        Ok(ScopeSet(scopes))
    }
}

impl fmt::Display for AtprotoScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AtprotoScope {
    type Err = UnknownScope;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "atproto" => Ok(AtprotoScope::Atproto),
            "transition:generic" => Ok(AtprotoScope::TransitionGeneric),
            "transition:chat.bsky" => Ok(AtprotoScope::TransitionChatBsky),
            other => Err(UnknownScope(other.to_string())),
        }
    }
}

/// Why a scope string failed to parse. Distinct variants so the authorize /
/// PAR endpoints can surface a precise `invalid_scope` reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeParseError {
    /// No scope tokens at all.
    Empty,
    /// The required base `atproto` scope was absent.
    MissingBase,
    /// A token outside the closed vocabulary.
    Unknown(String),
}

impl fmt::Display for ScopeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScopeParseError::Empty => f.write_str("scope must name at least one scope"),
            ScopeParseError::MissingBase => {
                f.write_str("scope must include the base 'atproto' scope")
            }
            ScopeParseError::Unknown(s) => write!(f, "unknown atproto scope: {s}"),
        }
    }
}

impl std::error::Error for ScopeParseError {}

/// An ordered, de-duplicated, validated set of atproto scopes.
///
/// Constructed only via [`AtprotoScope::parse_set`], so a `ScopeSet` always
/// names the base `atproto` scope and contains no unknown or duplicate tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeSet(Vec<AtprotoScope>);

impl ScopeSet {
    /// The canonical space-separated spelling — what gets persisted on the
    /// authorization request and the issued `token` row, and echoed in the
    /// token response. Re-canonicalising on the way in means stored scope
    /// strings are normalised regardless of client whitespace/duplication.
    pub fn to_canonical_string(&self) -> String {
        self.0
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl fmt::Display for ScopeSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_canonical_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_str_recognises_each_scope() {
        assert_eq!("atproto".parse(), Ok(AtprotoScope::Atproto));
        assert_eq!(
            "transition:generic".parse(),
            Ok(AtprotoScope::TransitionGeneric)
        );
        assert_eq!(
            "transition:chat.bsky".parse(),
            Ok(AtprotoScope::TransitionChatBsky)
        );
        assert_eq!(
            "atproto:repo".parse::<AtprotoScope>(),
            Err(UnknownScope("atproto:repo".to_string()))
        );
    }

    #[test]
    fn parse_set_canonicalises_and_dedups() {
        let set = AtprotoScope::parse_set("atproto  transition:generic atproto").unwrap();
        // Whitespace + duplicates collapse to a normalised canonical string,
        // in insertion order.
        assert_eq!(set.to_canonical_string(), "atproto transition:generic");
        // The base scope is present; an un-requested scope is not.
        assert!(set.to_canonical_string().contains("atproto"));
        assert!(!set.to_canonical_string().contains("chat.bsky"));
    }

    #[test]
    fn parse_set_requires_base_scope() {
        assert_eq!(
            AtprotoScope::parse_set("transition:generic"),
            Err(ScopeParseError::MissingBase)
        );
    }

    #[test]
    fn parse_set_rejects_empty_and_unknown() {
        assert_eq!(AtprotoScope::parse_set("   "), Err(ScopeParseError::Empty));
        assert_eq!(
            AtprotoScope::parse_set("atproto bogus"),
            Err(ScopeParseError::Unknown("bogus".to_string()))
        );
    }

    #[test]
    fn all_is_canonically_ordered() {
        let all = AtprotoScope::all();
        assert_eq!(all[0].as_str(), "atproto");
        assert_eq!(all.len(), 3);
    }
}
