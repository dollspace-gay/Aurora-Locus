//! Runtime route enumeration substrate (Arc 8, chainlink #54).
//!
//! Replaced the hand-curated capability list that previously
//! lived at `aurora_capability_families` /
//! `aurora_capability_extensions` (removed in Step 3) with a
//! registry populated at route registration time and queried by
//! `describe_capabilities` at request time. Step 1 built this
//! substrate; Step 2 migrated the registrations; Step 3
//! reimplemented `describe_capabilities` against the registry
//! and removed the hand-curated lists.
//!
//! Per V04_DESIGN.md §7.3.2 + §7.3.3:
//!
//! - [`RouteEntry`] captures per-route capability metadata
//!   (family, version, omission flag, extensions, registration
//!   order).
//! - [`RouteRegistry`] holds the entries and exposes
//!   advertisement-oriented query methods (filter omitted,
//!   group by family, union extensions). Query methods
//!   preserve **declaration-order** for accidental orderings per
//!   Step 0 Q5 disposition (a) — byte-identical wire output is
//!   the §7.3.4 lock.
//! - [`Family`] is the typed family identifier — enum so typos
//!   are compile errors; the wire-format string lives in a
//!   single `Display` impl.
//! - [`CapsBuilder`] is the metadata builder used at route
//!   registration sites (Step 2).
//! - [`RouteRegistryBuilder`] holds both the accumulating
//!   `Router` and the accumulating `Vec<RouteEntry>`;
//!   `.build()` returns the finalized pair.
//! - [`ADMIN_TIER_PATH_REGEX`] is the verified admin-tier path
//!   filter — Step 0 Q6 verified the regex against the actual
//!   axum route table and added `ops` to the alternation. Single
//!   shared constant per V04_DESIGN.md §7.3.6's
//!   "Shared-constant requirement."

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use axum::http::Method;
use axum::routing::MethodRouter;
use axum::Router;
use regex::Regex;

/// Admin-tier path filter — verified by Step 0 Q6 against the
/// actual axum route table. Routes matching this regex are
/// candidates for the [`RouteRegistry`]; routes not matching are
/// out-of-admin-scope (Step 0 Q6 List C classification).
///
/// The `ops` namespace was added during Step 0 Q6 Phase 6a —
/// admin-tier by authority and advertised in the existing
/// curated list, but originally missed by V04_DESIGN.md §7.3.6's
/// starting regex.
///
/// Defined once here per V04_DESIGN.md §7.3.6's "Shared-constant
/// requirement"; every consumer imports this constant. No second
/// copy of the regex exists anywhere in the tree.
pub const ADMIN_TIER_PATH_REGEX: &str =
    r"^/xrpc/tools\.aurora\.(admin|moderator|superadmin|ops)(\.|$)";

/// Compiled [`ADMIN_TIER_PATH_REGEX`] cached lazily on first
/// use. Returns a `&'static Regex` so callers don't recompile
/// on every match attempt.
pub fn admin_tier_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(ADMIN_TIER_PATH_REGEX).expect("admin-tier regex is well-formed")
    })
}

/// Canonical wire ordering for the flat extensions list emitted
/// by `describe_capabilities` (Arc 8 Step 3 — the registry-driven
/// handler reads `RouteRegistry::advertised_extensions`, which
/// filters this constant by the present-set).
///
/// # Versioning contract
///
/// Per `docs/V03_DESIGN.md` §6.3.1: capability strings follow the pattern `<kebab-family>-v<integer>`.
/// Kebab-case family name, hyphen, lowercase `v`, integer
/// version. Breaking changes ship as a NEW version suffix (e.g.
/// `subject-context-v2`); the OLD version is removed only after
/// the new version has shipped and consumers have had time to
/// migrate. Bumping the integer is the wire signal that breaking
/// change has landed.
///
/// Two §8.15 capabilities are intentionally omitted because the
/// corresponding endpoints are not yet shipped:
///   - `invite-lineage-v1`     (no shipped endpoint as of v0.4)
///   - `reporter-context-v1`   (no shipped endpoint as of v0.4)
///
/// Add them here when their handlers land.
///
/// The snapshot test `describe_capabilities_snapshot` in
/// `src/api/admin.rs` pins the wire-format byte-for-byte; the
/// contract-phrase test in `tests/contract_phrases.rs` pins this
/// doc comment's `<kebab-family>-v<integer>` phrase. Together they
/// declare both the structural shape and the versioning rule.
///
/// # Why a wire-order constant instead of registration-order union
///
/// Step 0 Q8 Part (c) claimed registration-order would reproduce
/// the existing wire output. That claim was wrong, and Step 2's
/// migration surfaced three concrete mismatches:
///
/// - `instance-metrics-v1` (Phase 2.3.8, on `ops.getInstanceMetrics`)
///   is registered first among admin-tier routes, but appears
///   fifth in wire output — after Phase 3.3's moderator
///   extensions — because the wire order tracks phase
///   *introduction* order, not route *registration* order.
/// - `subject-context-v1` (Phase 3.2, on
///   `moderator.getSubjectContext`) appears first in wire
///   output but its handler is registered fourth within the
///   moderator block (after `queryEvents`/`getEvent`/
///   `queryStatuses`).
/// - `moderation-metrics-v1` (Phase 3.7) precedes `queue-stats-v1`
///   in wire output, but the corresponding `getQueueStats`
///   handler is registered before `getModerationMetrics` —
///   another phase-vs-registration swap.
///
/// Encoding the wire order here decouples the (free) per-family
/// route registration order from the (pinned) wire-format
/// ordering. `debug_assert!` in `advertised_extensions` catches
/// drift if a registered route attributes an extension that
/// isn't in this constant.
pub const WIRE_EXTENSION_ORDER: &[&str] = &[
    // Phase 3.2 — capability probe ships alongside getSubjectContext.
    "subject-context-v1",
    // Phase 3.3 — moderator-tier reads.
    "moderator-activity-v1",
    "subject-history-v1",
    // Phase 3.4 — appeals reads.
    "appeals-v1",
    // Phase 2.3.8 — operator instance metrics.
    "instance-metrics-v1",
    // Phase 3.5 — emitEvent unified action surface + batch suite.
    "mod-events-emit-v1",
    "batch-takedown-v1",
    "trigger-password-reset-v1",
    // Phase 3.7 — moderation aggregations.
    "moderation-metrics-v1",
    "queue-stats-v1",
    // Phase 3.8 — hash-chained audit + forensic export.
    "audit-trail-v1",
    "forensic-export-v1",
    // Phase 3.9 — live event tail (HTTP-polling-driven in v0.2).
    "mod-events-stream-v1",
    // Phase 3.10 — runtime settings read/write.
    "runtime-settings-v1",
];

/// Typed family identifier — enum so typos are compile errors;
/// the wire-format string lives in a single [`Display`] impl.
///
/// Declaration order also serves as the `Ord` ordering used by
/// [`RouteRegistry::advertised_by_family`]'s `BTreeMap`. The
/// names match the four namespaces Step 0 Q6 verified against
/// the route table:
///
/// - `Admin` → `tools.aurora.admin`
/// - `Moderator` → `tools.aurora.moderator`
/// - `Ops` → `tools.aurora.ops`
/// - `SuperAdmin` → `tools.aurora.superadmin`
///
/// Alphabetical Display strings (`tools.aurora.admin`,
/// `.moderator`, `.ops`, `.superadmin`) coincide with the
/// `Ord`-derived enum order, so `BTreeMap<Family, _>` iteration
/// emits namespaces in the order the existing wire snapshot at
/// `src/api/admin.rs:7329-7438` expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Family {
    Admin,
    Moderator,
    Ops,
    SuperAdmin,
}

impl Family {
    /// Maps a [`Family`] to the broader authority class for
    /// per-request authorization. `Ops` routes are admin-tier
    /// by authority (mix of AdminServer scope + Admin+ at
    /// handler), so they collapse to [`FamilyKind::Admin`].
    pub fn kind(&self) -> FamilyKind {
        match self {
            Family::Admin => FamilyKind::Admin,
            Family::Moderator => FamilyKind::Moderator,
            Family::Ops => FamilyKind::Admin,
            Family::SuperAdmin => FamilyKind::SuperAdmin,
        }
    }

    /// The wire-format namespace string for this family.
    /// Equivalent to `format!("{}", family)` but exposed
    /// directly for use in builders where allocation matters.
    pub fn namespace(&self) -> &'static str {
        match self {
            Family::Admin => "tools.aurora.admin",
            Family::Moderator => "tools.aurora.moderator",
            Family::Ops => "tools.aurora.ops",
            Family::SuperAdmin => "tools.aurora.superadmin",
        }
    }
}

impl std::fmt::Display for Family {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.namespace())
    }
}

/// Authority class for per-route authorization grouping.
///
/// Distinct from [`Family`] because some families share the same
/// authority tier (e.g., `Admin` and `Ops` both require admin-
/// tier auth at the namespace middleware, with finer role
/// checks at handler level). `Public` is reserved for non-admin
/// routes if they ever enter the registry — none do today per
/// Step 0 Q6's List C exclusion of public XRPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FamilyKind {
    Admin,
    Moderator,
    SuperAdmin,
    Public,
}

/// Per-route capability metadata captured at registration time.
#[derive(Debug, Clone)]
pub struct RouteEntry {
    /// Full XRPC path, e.g. `/xrpc/tools.aurora.admin.emitEvent`.
    pub path: String,
    /// HTTP methods the route accepts. v0.4 leaves this empty —
    /// `axum::routing::MethodRouter` doesn't expose its accepted
    /// methods publicly, and the current `describe_capabilities`
    /// wire output doesn't include methods. Future cycles may
    /// populate this if a consumer needs it (v0.6 candidate).
    pub methods: Vec<Method>,
    /// The family this route belongs to. Family Display string
    /// becomes the namespace key in `describe_capabilities`.
    pub family: Family,
    /// Capability version. Used by the `<kebab-family>-v<integer>`
    /// versioning convention (V04_DESIGN.md §7.3.2). v0.4 routes
    /// all use version 1; v2 ships when a breaking change lands.
    pub version: u32,
    /// True iff this route is intentionally not advertised
    /// (§8.15 omission policy). Routes marked omitted are
    /// filtered out of [`RouteRegistry::advertised_entries`].
    pub omitted: bool,
    /// Capability extension strings this route contributes to
    /// the global extensions list. Empty for routes that don't
    /// contribute extensions.
    pub extensions: Vec<String>,
    /// Declaration order — the ordinal assigned at registration
    /// time. Used to preserve byte-identical wire output per
    /// Step 0 Q5 disposition (a) for accidental orderings.
    pub registration_order: u32,
}

/// Builder for [`RouteEntry`]'s capability metadata. Used at
/// route registration sites via
/// [`RouteRegistryBuilder::route_with_caps`].
///
/// Fluent shape: `CapsBuilder::new(family, version)
/// .extensions([...]).omitted()`. Each setter consumes and
/// returns `self` so chained calls stay readable.
#[derive(Debug, Clone)]
pub struct CapsBuilder {
    family: Family,
    version: u32,
    omitted: bool,
    extensions: Vec<String>,
}

impl CapsBuilder {
    /// Start a new metadata builder for the given family and
    /// version.
    pub fn new(family: Family, version: u32) -> Self {
        Self {
            family,
            version,
            omitted: false,
            extensions: Vec::new(),
        }
    }

    /// Mark this route as §8.15-omitted — present structurally
    /// but intentionally not advertised. Step 0 Q6 found no
    /// existing routes need this flag (List A was empty), but
    /// the API is in place for future cycles or for
    /// reintroducing omitted-capability handlers.
    pub fn omitted(mut self) -> Self {
        self.omitted = true;
        self
    }

    /// Attach extension strings this route contributes to the
    /// global `extensions` list. Accepts anything convertible
    /// to `String` (string literals, `&str`, `String`).
    pub fn extensions<I, S>(mut self, extensions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.extensions = extensions.into_iter().map(Into::into).collect();
        self
    }

    /// Read accessors for the values accumulated so far.
    /// Tests + the builder use these; not part of the public
    /// route-registration surface.
    pub fn family(&self) -> Family {
        self.family
    }
    pub fn version(&self) -> u32 {
        self.version
    }
    pub fn is_omitted(&self) -> bool {
        self.omitted
    }
    pub fn extension_strings(&self) -> &[String] {
        &self.extensions
    }
}

/// Registry of admin-tier routes for capability advertisement.
///
/// Populated at startup by [`RouteRegistryBuilder`] and consumed
/// by `describe_capabilities` at request time. The registry's
/// query methods preserve declaration-order for accidental
/// orderings per Step 0 Q5 disposition (a) — byte-identical
/// wire output is the §7.3.4 lock.
#[derive(Debug, Default)]
pub struct RouteRegistry {
    entries: Vec<RouteEntry>,
}

impl RouteRegistry {
    /// Construct a registry from an explicit entry list. Used
    /// by [`RouteRegistryBuilder::build`] and by tests that
    /// stage fixed data.
    pub fn from_entries(entries: Vec<RouteEntry>) -> Self {
        Self { entries }
    }

    /// All entries — advertised AND omitted. Use
    /// [`Self::advertised_entries`] for the wire-output-facing
    /// view.
    pub fn entries(&self) -> &[RouteEntry] {
        &self.entries
    }

    /// True iff the registry has no entries. Useful for
    /// asserting the empty default before Step 2's migration
    /// populates the registry.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of entries (advertised + omitted).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Iterator over entries that are NOT §8.15-omitted —
    /// the set `describe_capabilities` advertises.
    pub fn advertised_entries(&self) -> impl Iterator<Item = &RouteEntry> {
        self.entries.iter().filter(|e| !e.omitted)
    }

    /// Group advertised entries by family. Family ordering in
    /// the returned `BTreeMap` is alphabetical (the four
    /// `Family` variants' `Ord` order matches their `Display`
    /// strings — `admin` < `moderator` < `ops` < `superadmin`).
    /// Endpoint ordering within each family follows
    /// `registration_order` ascending — the byte-identical
    /// frozen order per Step 0 Q5 disposition (a).
    pub fn advertised_by_family(&self) -> BTreeMap<Family, Vec<&RouteEntry>> {
        let mut map: BTreeMap<Family, Vec<&RouteEntry>> = BTreeMap::new();
        for entry in self.advertised_entries() {
            map.entry(entry.family).or_default().push(entry);
        }
        for entries in map.values_mut() {
            entries.sort_by_key(|e| e.registration_order);
        }
        map
    }

    /// Flat extensions list for `describe_capabilities`,
    /// emitted in [`WIRE_EXTENSION_ORDER`] and filtered to the
    /// set of extensions actually attributed to advertised
    /// routes.
    ///
    /// Step 0 Q8 Part (c)'s original plan was a
    /// registration-order union; Step 2's migration surfaced
    /// the mismatch (instance-metrics-v1 / subject-context-v1
    /// / moderation-metrics-v1 / queue-stats-v1 all sit at
    /// positions that disagree with their handlers'
    /// registration order — see [`WIRE_EXTENSION_ORDER`]'s
    /// docstring for the three cases). Encoding the wire order
    /// in a separate constant keeps the snapshot byte-identical
    /// without forcing routes to be registered in a less
    /// natural order.
    ///
    /// `debug_assert!` panics in test/dev builds if a registered
    /// route attributes an extension that isn't in
    /// [`WIRE_EXTENSION_ORDER`] — surface drift loudly rather
    /// than silently dropping the string at runtime.
    pub fn advertised_extensions(&self) -> Vec<String> {
        let present: std::collections::HashSet<&str> = self
            .advertised_entries()
            .flat_map(|e| e.extensions.iter().map(String::as_str))
            .collect();

        if cfg!(debug_assertions) {
            for ext in &present {
                debug_assert!(
                    WIRE_EXTENSION_ORDER.contains(ext),
                    "extension {:?} attributed by a registered route is not \
                     present in WIRE_EXTENSION_ORDER — add it to the constant \
                     in declaration position or fix the attribution",
                    ext
                );
            }
        }

        WIRE_EXTENSION_ORDER
            .iter()
            .filter(|ext| present.contains(**ext))
            .map(|s| (*s).to_string())
            .collect()
    }
}

/// Startup-time builder that registers routes against both an
/// `axum::Router` and a parallel [`RouteRegistry`]. `.build()`
/// returns the finalised pair; `AppContext::new` will consume
/// the registry once Step 2 migrates the registrations.
///
/// The typestate pattern (Step 0 OQ kickoff §7.3.2) is captured
/// implicitly: callers thread the same builder through their
/// `.route_with_caps(...)` / `.route(...)` chain, then call
/// `.build()` once. There's no separate "open" vs "closed"
/// state type because all setters consume `self`; the builder
/// can't be reused after `.build()`.
pub struct RouteRegistryBuilder<S = crate::context::AppContext> {
    router: Router<S>,
    entries: Vec<RouteEntry>,
}

/// Construct an empty [`RouteRegistryBuilder`] parameterised on
/// the application's state type. Matches `Router::new()`'s
/// shape so registration sites don't have to fight type
/// inference.
pub fn aurora_route_builder<S>() -> RouteRegistryBuilder<S>
where
    S: Clone + Send + Sync + 'static,
{
    RouteRegistryBuilder {
        router: Router::new(),
        entries: Vec::new(),
    }
}

impl<S> RouteRegistryBuilder<S>
where
    S: Clone + Send + Sync + 'static,
{
    /// Register a route AND record its capability metadata.
    /// Step 2's primary migration site replaces every
    /// `.route(path, get|post(handler))` in `admin::routes()`
    /// with this call.
    pub fn route_with_caps(
        mut self,
        path: &str,
        handler: MethodRouter<S>,
        caps: CapsBuilder,
    ) -> Self {
        let entry = RouteEntry {
            path: path.to_string(),
            methods: Vec::new(),
            family: caps.family,
            version: caps.version,
            omitted: caps.omitted,
            extensions: caps.extensions,
            registration_order: self.entries.len() as u32,
        };
        self.entries.push(entry);
        self.router = self.router.route(path, handler);
        self
    }

    /// Pass-through for non-admin-tier routes that don't need
    /// capability metadata. Does NOT add a registry entry.
    /// Step 2 uses this for the `com.atproto.admin.*` family
    /// (Step 0 Q6 List C) and other out-of-admin-scope routes
    /// registered through the same builder.
    pub fn route(mut self, path: &str, handler: MethodRouter<S>) -> Self {
        self.router = self.router.route(path, handler);
        self
    }

    /// Merge another router (e.g., a sub-module's `routes()`
    /// output). Does NOT add registry entries — merged routers
    /// contribute their own entries via separate builders if
    /// they need registry tracking. Today no sub-router needs
    /// registry tracking (Step 0 Q1 found all admin-tier
    /// registration in one site); this method exists for
    /// forward-compat.
    pub fn merge(mut self, other: Router<S>) -> Self {
        self.router = self.router.merge(other);
        self
    }

    /// Finalise the builder. Consumes `self` and returns the
    /// constructed `Router` paired with an `Arc<RouteRegistry>`
    /// that `AppContext` parks for handler access.
    pub fn build(self) -> (Router<S>, Arc<RouteRegistry>) {
        let registry = RouteRegistry::from_entries(self.entries);
        (self.router, Arc::new(registry))
    }

    /// Read accessor for the entries accumulated so far. Used
    /// by tests; not part of the production registration
    /// surface. Production code calls `.build()` to get the
    /// finalised registry.
    pub fn entries(&self) -> &[RouteEntry] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- Family ----------

    #[test]
    fn family_display_strings_match_namespace_prefixes() {
        // Mandatory per V04_DESIGN.md §7.3.2's "single
        // explicitly tested typo surface" — the Display impl
        // for Family is the only place the namespace strings
        // live; this test catches drift between the enum and
        // the wire format.
        assert_eq!(Family::Admin.to_string(), "tools.aurora.admin");
        assert_eq!(Family::Moderator.to_string(), "tools.aurora.moderator");
        assert_eq!(Family::Ops.to_string(), "tools.aurora.ops");
        assert_eq!(Family::SuperAdmin.to_string(), "tools.aurora.superadmin");
    }

    #[test]
    fn family_namespace_method_matches_display() {
        // The namespace() inherent method must agree with the
        // Display impl. They share an implementation via
        // delegation today; this test pins the contract so a
        // future refactor doesn't drift one without the other.
        for family in [Family::Admin, Family::Moderator, Family::Ops, Family::SuperAdmin] {
            assert_eq!(family.namespace(), family.to_string());
        }
    }

    #[test]
    fn family_kind_assignments() {
        assert_eq!(Family::Admin.kind(), FamilyKind::Admin);
        assert_eq!(Family::Moderator.kind(), FamilyKind::Moderator);
        // Ops collapses to Admin authority class.
        assert_eq!(Family::Ops.kind(), FamilyKind::Admin);
        assert_eq!(Family::SuperAdmin.kind(), FamilyKind::SuperAdmin);
    }

    #[test]
    fn family_ord_matches_namespace_alphabetical() {
        // `BTreeMap<Family, _>` iterates in `Ord` order. The
        // existing wire snapshot expects namespaces in
        // alphabetical order (admin, moderator, ops,
        // superadmin); this test pins that the enum's `Ord`
        // matches.
        let mut families = vec![
            Family::SuperAdmin,
            Family::Ops,
            Family::Admin,
            Family::Moderator,
        ];
        families.sort();
        assert_eq!(
            families,
            vec![Family::Admin, Family::Moderator, Family::Ops, Family::SuperAdmin]
        );
    }

    // ---------- CapsBuilder ----------

    #[test]
    fn caps_builder_starts_with_no_omission_no_extensions() {
        let caps = CapsBuilder::new(Family::Admin, 1);
        assert_eq!(caps.family(), Family::Admin);
        assert_eq!(caps.version(), 1);
        assert!(!caps.is_omitted());
        assert!(caps.extension_strings().is_empty());
    }

    #[test]
    fn caps_builder_omitted_flips_the_flag() {
        let caps = CapsBuilder::new(Family::Admin, 1).omitted();
        assert!(caps.is_omitted());
    }

    #[test]
    fn caps_builder_extensions_accumulates() {
        let caps = CapsBuilder::new(Family::Moderator, 1)
            .extensions(["audit-trail-v1", "subject-history-v1"]);
        assert_eq!(
            caps.extension_strings(),
            &["audit-trail-v1".to_string(), "subject-history-v1".to_string()]
        );
    }

    #[test]
    fn caps_builder_chained_calls_preserve_values() {
        let caps = CapsBuilder::new(Family::Admin, 2)
            .extensions(["batch-takedown-v1"])
            .omitted();
        assert_eq!(caps.family(), Family::Admin);
        assert_eq!(caps.version(), 2);
        assert!(caps.is_omitted());
        assert_eq!(caps.extension_strings(), &["batch-takedown-v1".to_string()]);
    }

    // ---------- ADMIN_TIER_PATH_REGEX ----------

    #[test]
    fn admin_tier_regex_matches_all_four_namespaces() {
        let regex = admin_tier_regex();
        assert!(regex.is_match("/xrpc/tools.aurora.admin.emitEvent"));
        assert!(regex.is_match("/xrpc/tools.aurora.moderator.queryEvents"));
        assert!(regex.is_match("/xrpc/tools.aurora.ops.getStats"));
        assert!(regex.is_match("/xrpc/tools.aurora.superadmin.grantRole"));
    }

    #[test]
    fn admin_tier_regex_matches_nested_sub_namespaces() {
        // §7.3.6 explicitly covers nested namespaces. A
        // hypothetical `tools.aurora.admin.runtimeConfig.X`
        // path matches because the alternation accepts any
        // suffix after `admin`.
        let regex = admin_tier_regex();
        assert!(regex.is_match("/xrpc/tools.aurora.admin.runtimeConfig.setSetting"));
        assert!(regex.is_match("/xrpc/tools.aurora.ops.sequencer.pause"));
    }

    #[test]
    fn admin_tier_regex_rejects_out_of_scope_paths() {
        // Step 0 Q6 List C examples — these MUST NOT match.
        let regex = admin_tier_regex();

        // bsky-PDS-compat admin namespace (advertised separately
        // via lexicons, not the Aurora registry).
        assert!(!regex.is_match("/xrpc/com.atproto.admin.getUsers"));
        assert!(!regex.is_match("/xrpc/com.atproto.admin.takedownAccount"));

        // describeCapabilities itself — meta-endpoint that
        // describes the registry; can't be in the registry it
        // describes.
        assert!(!regex.is_match("/xrpc/tools.aurora.describeCapabilities"));

        // Public XRPC.
        assert!(!regex.is_match("/xrpc/com.atproto.server.createSession"));
        assert!(!regex.is_match("/xrpc/com.atproto.sync.subscribeRepos"));

        // Non-XRPC operational endpoints.
        assert!(!regex.is_match("/health"));
        assert!(!regex.is_match("/.well-known/did.json"));
        assert!(!regex.is_match("/metrics"));
        assert!(!regex.is_match("/admin/index.html"));
        assert!(!regex.is_match("/oauth/authorize"));
    }

    #[test]
    fn admin_tier_regex_rejects_lookalike_paths() {
        // Paths that share a prefix or share segments with the
        // admin-tier namespaces but aren't actually in scope.
        // Defends against accidental over-matching.
        let regex = admin_tier_regex();

        assert!(!regex.is_match("/xrpc/tools.auroramin.foo"));
        assert!(!regex.is_match("/xrpc/tools.aurora"));
        assert!(!regex.is_match("/xrpc/tools.aurora."));
        // tools.aurora.<other-namespace>.* — e.g.,
        // describeCapabilities or hypothetical
        // tools.aurora.session.* — must not match.
        assert!(!regex.is_match("/xrpc/tools.aurora.describeCapabilities"));
        assert!(!regex.is_match("/xrpc/tools.aurora.session.refresh"));
    }

    #[test]
    fn admin_tier_regex_accepts_bare_namespace_paths() {
        // The regex's `(\.|$)` group accepts either a dot
        // (subnamespace) or end-of-string (bare namespace).
        // Bare paths like `/xrpc/tools.aurora.admin` (without a
        // trailing endpoint name) match — handy for future
        // hypothetical "root-of-namespace" endpoints, and
        // doesn't cause false negatives if such a route ever
        // appears.
        let regex = admin_tier_regex();
        assert!(regex.is_match("/xrpc/tools.aurora.admin"));
        assert!(regex.is_match("/xrpc/tools.aurora.moderator"));
        assert!(regex.is_match("/xrpc/tools.aurora.ops"));
        assert!(regex.is_match("/xrpc/tools.aurora.superadmin"));
    }

    // ---------- RouteRegistry ----------

    fn fixture_entry(
        path: &str,
        family: Family,
        order: u32,
        extensions: Vec<&str>,
        omitted: bool,
    ) -> RouteEntry {
        RouteEntry {
            path: path.to_string(),
            methods: Vec::new(),
            family,
            version: 1,
            omitted,
            extensions: extensions.into_iter().map(String::from).collect(),
            registration_order: order,
        }
    }

    #[test]
    fn registry_empty_default_has_no_entries() {
        let registry = RouteRegistry::default();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert_eq!(registry.advertised_entries().count(), 0);
        assert!(registry.advertised_by_family().is_empty());
        assert!(registry.advertised_extensions().is_empty());
    }

    #[test]
    fn registry_advertised_entries_filters_omitted() {
        let registry = RouteRegistry::from_entries(vec![
            fixture_entry("/xrpc/tools.aurora.admin.emitEvent", Family::Admin, 0, vec![], false),
            fixture_entry("/xrpc/tools.aurora.admin.hidden", Family::Admin, 1, vec![], true),
            fixture_entry("/xrpc/tools.aurora.moderator.queryEvents", Family::Moderator, 2, vec![], false),
        ]);
        let advertised: Vec<_> = registry.advertised_entries().collect();
        assert_eq!(advertised.len(), 2);
        assert_eq!(advertised[0].path, "/xrpc/tools.aurora.admin.emitEvent");
        assert_eq!(advertised[1].path, "/xrpc/tools.aurora.moderator.queryEvents");
    }

    #[test]
    fn registry_advertised_by_family_groups_and_orders() {
        // Register entries out of alphabetical family order to
        // verify the BTreeMap-driven family ordering.
        let registry = RouteRegistry::from_entries(vec![
            fixture_entry("/xrpc/tools.aurora.superadmin.grantRole", Family::SuperAdmin, 0, vec![], false),
            fixture_entry("/xrpc/tools.aurora.admin.emitEvent", Family::Admin, 1, vec![], false),
            fixture_entry("/xrpc/tools.aurora.moderator.getEvent", Family::Moderator, 2, vec![], false),
            fixture_entry("/xrpc/tools.aurora.admin.batchTakedown", Family::Admin, 3, vec![], false),
            fixture_entry("/xrpc/tools.aurora.ops.getStats", Family::Ops, 4, vec![], false),
        ]);
        let grouped = registry.advertised_by_family();
        // BTreeMap iteration: Admin, Moderator, Ops, SuperAdmin.
        let families: Vec<_> = grouped.keys().copied().collect();
        assert_eq!(
            families,
            vec![Family::Admin, Family::Moderator, Family::Ops, Family::SuperAdmin]
        );
        // Admin family has two entries in registration_order (1 < 3).
        let admin_entries = grouped.get(&Family::Admin).unwrap();
        assert_eq!(admin_entries.len(), 2);
        assert_eq!(admin_entries[0].path, "/xrpc/tools.aurora.admin.emitEvent");
        assert_eq!(admin_entries[1].path, "/xrpc/tools.aurora.admin.batchTakedown");
    }

    #[test]
    fn registry_advertised_by_family_skips_omitted() {
        let registry = RouteRegistry::from_entries(vec![
            fixture_entry("/xrpc/tools.aurora.admin.live", Family::Admin, 0, vec![], false),
            fixture_entry("/xrpc/tools.aurora.admin.omitted", Family::Admin, 1, vec![], true),
        ]);
        let grouped = registry.advertised_by_family();
        assert_eq!(grouped.get(&Family::Admin).unwrap().len(), 1);
    }

    #[test]
    fn registry_advertised_extensions_emits_in_wire_order_not_registration_order() {
        // Register entries in registration_order that DOES NOT
        // match the WIRE_EXTENSION_ORDER (e.g.,
        // moderator-activity-v1 registered first, but
        // subject-context-v1 must come first in wire output).
        // This is exactly the live Phase 3.2/3.3 mismatch — see
        // the `WIRE_EXTENSION_ORDER` docstring.
        let registry = RouteRegistry::from_entries(vec![
            fixture_entry(
                "/a",
                Family::Moderator,
                0,
                vec!["moderator-activity-v1"],
                false,
            ),
            fixture_entry(
                "/b",
                Family::Moderator,
                1,
                vec!["subject-context-v1"],
                false,
            ),
            fixture_entry(
                "/c",
                Family::Moderator,
                2,
                vec!["subject-history-v1"],
                false,
            ),
            // Same extension on a later entry — present-set
            // dedup keeps wire output a single-occurrence list.
            fixture_entry(
                "/d",
                Family::Admin,
                3,
                vec!["mod-events-emit-v1", "subject-context-v1"],
                false,
            ),
        ]);
        let extensions = registry.advertised_extensions();
        assert_eq!(
            extensions,
            vec![
                "subject-context-v1".to_string(),
                "moderator-activity-v1".to_string(),
                "subject-history-v1".to_string(),
                "mod-events-emit-v1".to_string(),
            ]
        );
    }

    #[test]
    fn registry_advertised_extensions_skips_omitted_entries() {
        let registry = RouteRegistry::from_entries(vec![
            // `subject-context-v1` is in WIRE_EXTENSION_ORDER —
            // the present-set on the omitted entry must not
            // leak through to the wire output even though the
            // string is otherwise wire-known.
            fixture_entry("/a", Family::Admin, 0, vec!["mod-events-emit-v1"], false),
            fixture_entry("/b", Family::Admin, 1, vec!["subject-context-v1"], true),
        ]);
        let extensions = registry.advertised_extensions();
        assert_eq!(extensions, vec!["mod-events-emit-v1".to_string()]);
    }

    // ---------- RouteRegistryBuilder ----------

    // The builder methods take `axum::routing::MethodRouter`
    // values which require a concrete state type; the test
    // module uses a unit state for compactness.

    async fn dummy_handler() -> &'static str {
        "ok"
    }

    #[test]
    fn builder_starts_empty() {
        let builder: RouteRegistryBuilder<()> = aurora_route_builder();
        assert!(builder.entries().is_empty());
    }

    #[test]
    fn builder_route_with_caps_accumulates_entries() {
        let builder: RouteRegistryBuilder<()> = aurora_route_builder()
            .route_with_caps(
                "/xrpc/tools.aurora.admin.emitEvent",
                axum::routing::post(dummy_handler),
                CapsBuilder::new(Family::Admin, 1).extensions(["mod-events-emit-v1"]),
            )
            .route_with_caps(
                "/xrpc/tools.aurora.moderator.queryEvents",
                axum::routing::get(dummy_handler),
                CapsBuilder::new(Family::Moderator, 1).extensions(["moderator-activity-v1"]),
            );
        let entries = builder.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].registration_order, 0);
        assert_eq!(entries[0].family, Family::Admin);
        assert_eq!(entries[1].registration_order, 1);
        assert_eq!(entries[1].family, Family::Moderator);
    }

    #[test]
    fn builder_route_does_not_add_registry_entries() {
        let builder: RouteRegistryBuilder<()> = aurora_route_builder()
            .route("/health", axum::routing::get(dummy_handler))
            .route_with_caps(
                "/xrpc/tools.aurora.admin.emitEvent",
                axum::routing::post(dummy_handler),
                CapsBuilder::new(Family::Admin, 1),
            );
        // Only the `.route_with_caps` call added an entry.
        assert_eq!(builder.entries().len(), 1);
        assert_eq!(
            builder.entries()[0].path,
            "/xrpc/tools.aurora.admin.emitEvent"
        );
    }

    #[test]
    fn builder_build_finalises_registry_with_entries() {
        let builder: RouteRegistryBuilder<()> = aurora_route_builder()
            .route_with_caps(
                "/xrpc/tools.aurora.ops.getStats",
                axum::routing::get(dummy_handler),
                CapsBuilder::new(Family::Ops, 1),
            );
        let (_router, registry) = builder.build();
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.entries()[0].family, Family::Ops);
    }

    #[test]
    fn builder_omitted_caps_flow_through_to_registry() {
        let builder: RouteRegistryBuilder<()> = aurora_route_builder()
            .route_with_caps(
                "/xrpc/tools.aurora.admin.hidden",
                axum::routing::post(dummy_handler),
                CapsBuilder::new(Family::Admin, 1).omitted(),
            );
        let (_router, registry) = builder.build();
        assert_eq!(registry.advertised_entries().count(), 0);
        assert_eq!(registry.entries().len(), 1);
        assert!(registry.entries()[0].omitted);
    }
}
