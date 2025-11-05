/// OAuth 2.1 Scope System for ATProto
///
/// Implements fine-grained authorization scopes mapped to ATProto lexicons.
/// Scopes control what operations an OAuth token can perform.
///
/// Scope Hierarchy:
/// - atproto:* - Full access (admin/first-party apps only)
/// - atproto:read - Read-only access to all endpoints
/// - atproto:write - Write access (create, update, delete)
/// - atproto:repo.* - Repository operations
/// - atproto:repo.create - Create records
/// - atproto:repo.update - Update records
/// - atproto:repo.delete - Delete records
/// - atproto:identity.* - Identity operations
/// - atproto:admin.* - Administrative operations (highly privileged)
///
/// References:
/// - ATProto OAuth spec: https://atproto.com/specs/oauth

use crate::error::{PdsError, PdsResult};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

/// ATProto OAuth Scope
///
/// Represents a permission scope for OAuth tokens.
/// Scopes are hierarchical: `atproto:repo.*` includes `atproto:repo.create`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AtProtoScope {
    /// Full access to all endpoints (admin/first-party apps)
    All,

    /// Read-only access
    Read,

    /// Write access (create, update, delete)
    Write,

    /// Repository operations (all)
    RepoAll,

    /// Create repository records
    RepoCreate,

    /// Update repository records
    RepoUpdate,

    /// Delete repository records
    RepoDelete,

    /// List repository records
    RepoList,

    /// Get repository records
    RepoGet,

    /// Identity operations (all)
    IdentityAll,

    /// Update user profile
    IdentityUpdateProfile,

    /// Resolve DID
    IdentityResolveDid,

    /// Upload blobs
    BlobUpload,

    /// Delete blobs
    BlobDelete,

    /// Administrative operations (all) - highly privileged
    AdminAll,

    /// Moderation actions
    AdminModeration,

    /// Server management
    AdminServer,

    /// Custom scope (for forward compatibility)
    Custom(String),
}

impl AtProtoScope {
    /// Check if this scope includes another scope
    ///
    /// Implements hierarchical scope checking:
    /// - `All` includes everything
    /// - `RepoAll` includes `RepoCreate`, `RepoUpdate`, etc.
    /// - `Write` includes create/update/delete operations
    /// - `Read` includes get/list operations
    ///
    /// # Arguments
    /// * `other` - The scope to check for inclusion
    ///
    /// # Returns
    /// True if this scope includes the other scope
    pub fn includes(&self, other: &AtProtoScope) -> bool {
        match (self, other) {
            // All includes everything
            (AtProtoScope::All, _) => true,

            // Exact match
            (s1, s2) if s1 == s2 => true,

            // Write includes create/update/delete
            (AtProtoScope::Write, AtProtoScope::RepoCreate) => true,
            (AtProtoScope::Write, AtProtoScope::RepoUpdate) => true,
            (AtProtoScope::Write, AtProtoScope::RepoDelete) => true,
            (AtProtoScope::Write, AtProtoScope::BlobUpload) => true,
            (AtProtoScope::Write, AtProtoScope::BlobDelete) => true,

            // Read includes get/list/resolve operations
            (AtProtoScope::Read, AtProtoScope::RepoGet) => true,
            (AtProtoScope::Read, AtProtoScope::RepoList) => true,
            (AtProtoScope::Read, AtProtoScope::IdentityResolveDid) => true,

            // RepoAll includes all repo operations
            (AtProtoScope::RepoAll, AtProtoScope::RepoCreate) => true,
            (AtProtoScope::RepoAll, AtProtoScope::RepoUpdate) => true,
            (AtProtoScope::RepoAll, AtProtoScope::RepoDelete) => true,
            (AtProtoScope::RepoAll, AtProtoScope::RepoList) => true,
            (AtProtoScope::RepoAll, AtProtoScope::RepoGet) => true,

            // IdentityAll includes all identity operations
            (AtProtoScope::IdentityAll, AtProtoScope::IdentityUpdateProfile) => true,
            (AtProtoScope::IdentityAll, AtProtoScope::IdentityResolveDid) => true,

            // AdminAll includes all admin operations
            (AtProtoScope::AdminAll, AtProtoScope::AdminModeration) => true,
            (AtProtoScope::AdminAll, AtProtoScope::AdminServer) => true,

            // No inclusion
            _ => false,
        }
    }

    /// Check if this is a privileged scope requiring special authorization
    pub fn is_privileged(&self) -> bool {
        matches!(
            self,
            AtProtoScope::All
                | AtProtoScope::AdminAll
                | AtProtoScope::AdminModeration
                | AtProtoScope::AdminServer
        )
    }

    /// Get human-readable description of this scope
    pub fn description(&self) -> &'static str {
        match self {
            AtProtoScope::All => "Full access to all endpoints",
            AtProtoScope::Read => "Read-only access to all data",
            AtProtoScope::Write => "Create, update, and delete content",
            AtProtoScope::RepoAll => "Full access to repository operations",
            AtProtoScope::RepoCreate => "Create new records",
            AtProtoScope::RepoUpdate => "Update existing records",
            AtProtoScope::RepoDelete => "Delete records",
            AtProtoScope::RepoList => "List repository records",
            AtProtoScope::RepoGet => "Get repository records",
            AtProtoScope::IdentityAll => "Full access to identity operations",
            AtProtoScope::IdentityUpdateProfile => "Update user profile",
            AtProtoScope::IdentityResolveDid => "Resolve DID identifiers",
            AtProtoScope::BlobUpload => "Upload files and media",
            AtProtoScope::BlobDelete => "Delete files and media",
            AtProtoScope::AdminAll => "Full administrative access",
            AtProtoScope::AdminModeration => "Moderation actions",
            AtProtoScope::AdminServer => "Server management",
            AtProtoScope::Custom(_) => "Custom scope",
        }
    }

    /// Get scope category for UI grouping
    pub fn category(&self) -> &'static str {
        match self {
            AtProtoScope::All => "admin",
            AtProtoScope::Read | AtProtoScope::Write => "basic",
            AtProtoScope::RepoAll
            | AtProtoScope::RepoCreate
            | AtProtoScope::RepoUpdate
            | AtProtoScope::RepoDelete
            | AtProtoScope::RepoList
            | AtProtoScope::RepoGet => "repo",
            AtProtoScope::IdentityAll
            | AtProtoScope::IdentityUpdateProfile
            | AtProtoScope::IdentityResolveDid => "identity",
            AtProtoScope::BlobUpload | AtProtoScope::BlobDelete => "blob",
            AtProtoScope::AdminAll | AtProtoScope::AdminModeration | AtProtoScope::AdminServer => {
                "admin"
            }
            AtProtoScope::Custom(_) => "custom",
        }
    }
}

impl fmt::Display for AtProtoScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            AtProtoScope::All => "atproto:*",
            AtProtoScope::Read => "atproto:read",
            AtProtoScope::Write => "atproto:write",
            AtProtoScope::RepoAll => "atproto:repo.*",
            AtProtoScope::RepoCreate => "atproto:repo.create",
            AtProtoScope::RepoUpdate => "atproto:repo.update",
            AtProtoScope::RepoDelete => "atproto:repo.delete",
            AtProtoScope::RepoList => "atproto:repo.list",
            AtProtoScope::RepoGet => "atproto:repo.get",
            AtProtoScope::IdentityAll => "atproto:identity.*",
            AtProtoScope::IdentityUpdateProfile => "atproto:identity.updateProfile",
            AtProtoScope::IdentityResolveDid => "atproto:identity.resolveDid",
            AtProtoScope::BlobUpload => "atproto:blob.upload",
            AtProtoScope::BlobDelete => "atproto:blob.delete",
            AtProtoScope::AdminAll => "atproto:admin.*",
            AtProtoScope::AdminModeration => "atproto:admin.moderation",
            AtProtoScope::AdminServer => "atproto:admin.server",
            AtProtoScope::Custom(s) => s,
        };
        write!(f, "{}", s)
    }
}

impl FromStr for AtProtoScope {
    type Err = PdsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "atproto:*" => Ok(AtProtoScope::All),
            "atproto:read" => Ok(AtProtoScope::Read),
            "atproto:write" => Ok(AtProtoScope::Write),
            "atproto:repo.*" => Ok(AtProtoScope::RepoAll),
            "atproto:repo.create" => Ok(AtProtoScope::RepoCreate),
            "atproto:repo.update" => Ok(AtProtoScope::RepoUpdate),
            "atproto:repo.delete" => Ok(AtProtoScope::RepoDelete),
            "atproto:repo.list" => Ok(AtProtoScope::RepoList),
            "atproto:repo.get" => Ok(AtProtoScope::RepoGet),
            "atproto:identity.*" => Ok(AtProtoScope::IdentityAll),
            "atproto:identity.updateProfile" => Ok(AtProtoScope::IdentityUpdateProfile),
            "atproto:identity.resolveDid" => Ok(AtProtoScope::IdentityResolveDid),
            "atproto:blob.upload" => Ok(AtProtoScope::BlobUpload),
            "atproto:blob.delete" => Ok(AtProtoScope::BlobDelete),
            "atproto:admin.*" => Ok(AtProtoScope::AdminAll),
            "atproto:admin.moderation" => Ok(AtProtoScope::AdminModeration),
            "atproto:admin.server" => Ok(AtProtoScope::AdminServer),
            other => Ok(AtProtoScope::Custom(other.to_string())),
        }
    }
}

/// Scope set representing a collection of scopes
#[derive(Debug, Clone)]
pub struct ScopeSet {
    scopes: HashSet<AtProtoScope>,
}

impl ScopeSet {
    /// Create a new empty scope set
    pub fn new() -> Self {
        Self {
            scopes: HashSet::new(),
        }
    }

    /// Parse scopes from space-separated string
    ///
    /// # Arguments
    /// * `scope_str` - Space-separated scope string (e.g., "atproto:read atproto:write")
    ///
    /// # Returns
    /// Parsed scope set
    pub fn from_str(scope_str: &str) -> PdsResult<Self> {
        let scopes = scope_str
            .split_whitespace()
            .map(|s| AtProtoScope::from_str(s))
            .collect::<Result<HashSet<_>, _>>()?;

        Ok(Self { scopes })
    }

    /// Convert scope set to space-separated string
    pub fn to_string(&self) -> String {
        self.scopes
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Check if this set contains a specific scope (including hierarchical)
    ///
    /// # Arguments
    /// * `required` - The scope to check for
    ///
    /// # Returns
    /// True if this set contains or includes the required scope
    pub fn has_scope(&self, required: &AtProtoScope) -> bool {
        self.scopes.iter().any(|s| s.includes(required))
    }

    /// Check if this set has any of the required scopes
    pub fn has_any(&self, required: &[AtProtoScope]) -> bool {
        required.iter().any(|req| self.has_scope(req))
    }

    /// Check if this set has all of the required scopes
    pub fn has_all(&self, required: &[AtProtoScope]) -> bool {
        required.iter().all(|req| self.has_scope(req))
    }

    /// Get intersection with another scope set (for token refresh)
    ///
    /// Returns a new scope set containing only scopes present in both sets.
    pub fn intersect(&self, other: &ScopeSet) -> Self {
        let scopes = self
            .scopes
            .intersection(&other.scopes)
            .cloned()
            .collect();

        Self { scopes }
    }

    /// Add a scope to this set
    pub fn add(&mut self, scope: AtProtoScope) {
        self.scopes.insert(scope);
    }

    /// Check if this set contains any privileged scopes
    pub fn has_privileged_scopes(&self) -> bool {
        self.scopes.iter().any(|s| s.is_privileged())
    }

    /// Get all scopes in this set
    pub fn scopes(&self) -> Vec<&AtProtoScope> {
        self.scopes.iter().collect()
    }

    /// Get number of scopes
    pub fn len(&self) -> usize {
        self.scopes.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.scopes.is_empty()
    }
}

impl Default for ScopeSet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_parsing() {
        let scope = AtProtoScope::from_str("atproto:read").unwrap();
        assert_eq!(scope, AtProtoScope::Read);

        let scope = AtProtoScope::from_str("atproto:repo.create").unwrap();
        assert_eq!(scope, AtProtoScope::RepoCreate);

        let scope = AtProtoScope::from_str("custom:scope").unwrap();
        assert!(matches!(scope, AtProtoScope::Custom(_)));
    }

    #[test]
    fn test_scope_includes() {
        // All includes everything
        assert!(AtProtoScope::All.includes(&AtProtoScope::Read));
        assert!(AtProtoScope::All.includes(&AtProtoScope::RepoCreate));
        assert!(AtProtoScope::All.includes(&AtProtoScope::AdminAll));

        // Write includes create/update/delete
        assert!(AtProtoScope::Write.includes(&AtProtoScope::RepoCreate));
        assert!(AtProtoScope::Write.includes(&AtProtoScope::RepoUpdate));
        assert!(AtProtoScope::Write.includes(&AtProtoScope::RepoDelete));
        assert!(!AtProtoScope::Write.includes(&AtProtoScope::RepoGet));

        // Read includes get/list
        assert!(AtProtoScope::Read.includes(&AtProtoScope::RepoGet));
        assert!(AtProtoScope::Read.includes(&AtProtoScope::RepoList));
        assert!(!AtProtoScope::Read.includes(&AtProtoScope::RepoCreate));

        // RepoAll includes all repo operations
        assert!(AtProtoScope::RepoAll.includes(&AtProtoScope::RepoCreate));
        assert!(AtProtoScope::RepoAll.includes(&AtProtoScope::RepoGet));
    }

    #[test]
    fn test_scope_set_parsing() {
        let set = ScopeSet::from_str("atproto:read atproto:write").unwrap();
        assert_eq!(set.len(), 2);
        assert!(set.has_scope(&AtProtoScope::Read));
        assert!(set.has_scope(&AtProtoScope::Write));
    }

    #[test]
    fn test_scope_set_hierarchical() {
        let set = ScopeSet::from_str("atproto:write").unwrap();

        // Write should include create/update/delete
        assert!(set.has_scope(&AtProtoScope::RepoCreate));
        assert!(set.has_scope(&AtProtoScope::RepoUpdate));
        assert!(set.has_scope(&AtProtoScope::RepoDelete));

        // But not get/list (read operations)
        assert!(!set.has_scope(&AtProtoScope::RepoGet));
    }

    #[test]
    fn test_scope_set_intersection() {
        let set1 = ScopeSet::from_str("atproto:read atproto:write atproto:admin.*").unwrap();
        let set2 = ScopeSet::from_str("atproto:read atproto:repo.create").unwrap();

        let intersection = set1.intersect(&set2);
        assert_eq!(intersection.len(), 1);
        assert!(intersection.has_scope(&AtProtoScope::Read));
    }

    #[test]
    fn test_privileged_scopes() {
        assert!(AtProtoScope::All.is_privileged());
        assert!(AtProtoScope::AdminAll.is_privileged());
        assert!(!AtProtoScope::Read.is_privileged());
        assert!(!AtProtoScope::Write.is_privileged());
    }

    #[test]
    fn test_scope_to_string() {
        assert_eq!(AtProtoScope::Read.to_string(), "atproto:read");
        assert_eq!(AtProtoScope::RepoCreate.to_string(), "atproto:repo.create");
        assert_eq!(AtProtoScope::All.to_string(), "atproto:*");
    }

    #[test]
    fn test_scope_description() {
        assert_eq!(AtProtoScope::Read.description(), "Read-only access to all data");
        assert_eq!(
            AtProtoScope::RepoCreate.description(),
            "Create new records"
        );
    }

    #[test]
    fn test_scope_category() {
        assert_eq!(AtProtoScope::Read.category(), "basic");
        assert_eq!(AtProtoScope::RepoCreate.category(), "repo");
        assert_eq!(AtProtoScope::IdentityResolveDid.category(), "identity");
        assert_eq!(AtProtoScope::AdminAll.category(), "admin");
    }
}

/// Middleware helper functions for scope checking
///
/// These functions are used to protect API endpoints with scope requirements.

/// Check if the given scope string contains the required scope
///
/// Used in API endpoint handlers to verify OAuth token permissions.
///
/// # Arguments
/// * `token_scopes` - Space-separated scope string from the OAuth token
/// * `required` - The scope required for this operation
///
/// # Returns
/// Ok(()) if authorized, Err(PdsError::Forbidden) otherwise
///
/// # Example
/// ```ignore
/// require_scope(&token.scope, &AtProtoScope::RepoCreate)?;
/// ```
pub fn require_scope(token_scopes: &str, required: &AtProtoScope) -> PdsResult<()> {
    let scopes = ScopeSet::from_str(token_scopes)?;

    if scopes.has_scope(required) {
        Ok(())
    } else {
        Err(PdsError::Authorization(format!(
            "Insufficient scope: requires {}",
            required
        )))
    }
}

/// Check if the given scope string contains ANY of the required scopes
///
/// # Arguments
/// * `token_scopes` - Space-separated scope string from the OAuth token
/// * `required` - List of acceptable scopes (any one will do)
///
/// # Returns
/// Ok(()) if authorized, Err(PdsError::Forbidden) otherwise
pub fn require_any_scope(token_scopes: &str, required: &[AtProtoScope]) -> PdsResult<()> {
    let scopes = ScopeSet::from_str(token_scopes)?;

    if scopes.has_any(required) {
        Ok(())
    } else {
        let required_str = required
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join(", ");

        Err(PdsError::Authorization(format!(
            "Insufficient scope: requires one of [{}]",
            required_str
        )))
    }
}

/// Check if the given scope string contains ALL of the required scopes
///
/// # Arguments
/// * `token_scopes` - Space-separated scope string from the OAuth token
/// * `required` - List of required scopes (all must be present)
///
/// # Returns
/// Ok(()) if authorized, Err(PdsError::Forbidden) otherwise
pub fn require_all_scopes(token_scopes: &str, required: &[AtProtoScope]) -> PdsResult<()> {
    let scopes = ScopeSet::from_str(token_scopes)?;

    if scopes.has_all(required) {
        Ok(())
    } else {
        let required_str = required
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join(", ");

        Err(PdsError::Authorization(format!(
            "Insufficient scope: requires all of [{}]",
            required_str
        )))
    }
}

/// Map ATProto lexicon NSID to required scope
///
/// This function maps lexicon identifiers to their corresponding OAuth scopes.
/// Used to automatically determine what scope is needed for an XRPC endpoint.
///
/// # Arguments
/// * `nsid` - Lexicon NSID (e.g., "com.atproto.repo.createRecord")
///
/// # Returns
/// Required scope for this lexicon
pub fn lexicon_to_scope(nsid: &str) -> AtProtoScope {
    if nsid.starts_with("com.atproto.repo.create") || nsid.starts_with("app.bsky.feed.post") {
        AtProtoScope::RepoCreate
    } else if nsid.starts_with("com.atproto.repo.put") {
        AtProtoScope::RepoUpdate
    } else if nsid.starts_with("com.atproto.repo.delete") {
        AtProtoScope::RepoDelete
    } else if nsid.starts_with("com.atproto.repo.list") {
        AtProtoScope::RepoList
    } else if nsid.starts_with("com.atproto.repo.get")
        || nsid.starts_with("com.atproto.repo.describe")
    {
        AtProtoScope::RepoGet
    } else if nsid.starts_with("com.atproto.identity.resolve") {
        AtProtoScope::IdentityResolveDid
    } else if nsid.starts_with("com.atproto.identity.update") {
        AtProtoScope::IdentityUpdateProfile
    } else if nsid.starts_with("com.atproto.repo.uploadBlob") {
        AtProtoScope::BlobUpload
    } else if nsid.starts_with("com.atproto.admin") {
        AtProtoScope::AdminAll
    } else {
        // Default to read for unknown endpoints
        AtProtoScope::Read
    }
}
