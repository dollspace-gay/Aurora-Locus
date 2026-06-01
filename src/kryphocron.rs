//! v0.7 arc 1 — kryphocron substrate integration glue.
//!
//! This module owns the host-substrate state Aurora-Locus carries on
//! behalf of the kryphocron integration:
//!
//! - The build-time deny-error map populated from
//!   `kryphocron::KRYPHOCRON_LEXICON_REGISTRY`. Per v07_DESIGN.md §8,
//!   every registered NSID that lacks a dedicated endpoint receives a
//!   `KryphocronRecordNotYetSupported` mapping; in arc 1 ship state that
//!   is every NSID (no dedicated endpoints exist yet — arc 3+).
//! - The runtime probe used by [`crate::context::AppContext::new`] to
//!   force-initialise `kryphocron::lexicons()` at startup so any
//!   lexicon-parse failure surfaces loudly before the first request.
//!
//! Arc 1 ship state: the registered-NSID dispatcher branch in
//! `RepositoryManager::validate_write` is dead code because the
//! kryphocron-prefix early-deny fires first (Path A from the kickoff).
//! Arc 2 wires the `WriteOp.kryphocron_authorization` flag and adapts
//! the deny rule to bypass for legitimate dedicated-endpoint / cascade
//! origins; arc 3+ populates per-endpoint mappings that override the
//! arc 1 `NotYetSupported` defaults.

use std::collections::HashMap;

use uuid::Uuid;

use crate::actor_store::repository::WriteOpAction;
use crate::error::PdsError;

/// Re-export the substrate's capability-class discriminator. The
/// `KryphocronWriteAuthorization::DedicatedEndpoint` variant carries
/// this so audit emit can render `"capability_class": "user" | ...`
/// per v07_DESIGN.md §4 payload spec without Aurora-Locus owning a
/// parallel string enum.
pub use kryphocron::CapabilityClass;

/// Kryphocron deny-variant discriminator. Stored in the
/// `(nsid, action) → variant` map built from
/// `kryphocron::KRYPHOCRON_LEXICON_REGISTRY` at startup; consulted from
/// the dispatcher's deny-by-default branch.
///
/// In arc 1, every entry in the map is `NotYetSupported` (no dedicated
/// endpoints exist). Arc 3+ adds `RequiresDedicatedEndpoint`
/// entries with concrete `suggested_endpoint` strings per endpoint
/// registration.
#[derive(Clone, Debug)]
pub enum KryphocronDenyVariant {
    /// Per v07_DESIGN.md §8 lines 4836-4847. Defaulted in arc 1 for
    /// every registered NSID; arc 3+ overrides on a per-endpoint basis.
    NotYetSupported,
    /// Per v07_DESIGN.md §8 lines 4832-4836. Populated by the
    /// dedicated-endpoint registration code (arc 3+); arc 1 never
    /// produces this variant.
    #[allow(dead_code)]
    RequiresDedicatedEndpoint {
        suggested_endpoint: Option<String>,
    },
}

impl KryphocronDenyVariant {
    /// Convert this deny variant + the NSID it was looked up against
    /// into a [`PdsError`] for the dispatcher's `Err(...)` return.
    pub fn into_pds_error(self, nsid: &str) -> PdsError {
        match self {
            KryphocronDenyVariant::NotYetSupported => {
                PdsError::KryphocronRecordNotYetSupported {
                    nsid: nsid.to_string(),
                }
            }
            KryphocronDenyVariant::RequiresDedicatedEndpoint { suggested_endpoint } => {
                PdsError::KryphocronRecordRequiresDedicatedEndpoint {
                    nsid: nsid.to_string(),
                    suggested_endpoint,
                }
            }
        }
    }
}

/// Build the deny-error map from `kryphocron::KRYPHOCRON_LEXICON_REGISTRY`.
///
/// Per v07_DESIGN.md §8 lines 4849-4855: a static
/// `HashMap<(nsid, op), DenyError>` populated from two sources. Arc 1
/// only has source 2 (registry-without-dedicated-endpoint); arc 3+ adds
/// source 1 (per-endpoint overrides that fire before the automatic
/// derivation pass). Both populate the same map at the same startup
/// step.
///
/// Arc 1 implementation: walks the registry, fills `NotYetSupported`
/// for every (NSID, Create | Update | Delete) tuple. Operators wanting
/// to verify the surface can `cargo run -- list-kryphocron-nsids` once
/// that CLI ships (post-arc-1), or inspect the registry directly.
pub fn build_deny_map() -> HashMap<(String, WriteOpAction), KryphocronDenyVariant> {
    let mut map = HashMap::new();
    for entry in kryphocron::KRYPHOCRON_LEXICON_REGISTRY {
        let nsid = entry.nsid.to_string();
        for action in [
            WriteOpAction::Create,
            WriteOpAction::Update,
            WriteOpAction::Delete,
        ] {
            map.insert(
                (nsid.clone(), action),
                KryphocronDenyVariant::NotYetSupported,
            );
        }
    }
    map
}

/// Force `kryphocron::lexicons()` to initialise at startup rather than
/// at first dispatch. Any lexicon-parse failure surfaces loudly here
/// instead of mid-request.
///
/// Caller invokes this when `config.kryphocron.enabled == true`. When
/// the master switch is off, the registry stays uninitialised and the
/// dispatcher's master-switch-off branch never reaches it.
///
/// # Panics
///
/// Per the substrate's documented contract, `kryphocron::lexicons()`
/// panics if any embedded lexicon JSON fails to parse — the JSON is
/// vendored in-tree at the kryphocron-lexicons crate, so a panic here
/// is a substrate bug, not a host-input failure.
pub fn warm_lexicons() {
    let _ = kryphocron::lexicons();
}

/// Validate a record's shape against the kryphocron substrate's
/// compiled-in lexicon definitions.
///
/// The substrate accessor `kryphocron::lexicons()` returns a
/// `&'static proto_blue::lexicon::Lexicons` container holding every
/// `tools.kryphocron.*` LexiconDoc. This helper looks up the `#main`
/// def for the supplied NSID, asserts it's a `Record` user-type, and
/// delegates to `proto_blue::lexicon::validate_record`.
///
/// Per v07_DESIGN.md §6 line 3335 (and the substrate-touch-up
/// kickoff's two-export reality clarification): the registry tier
/// check answers "is this NSID known"; this helper answers "does the
/// record conform to the declared lexicon shape". Both gates must
/// pass before the bind pipeline runs.
///
/// In arc 1 ship state this helper is dead code because the
/// kryphocron-prefix early-deny fires before the registered-NSID
/// dispatcher branch (Path A from the kickoff). It is reachable in
/// arc 3+ when dedicated endpoints route registered NSIDs through
/// the `Ok(tier)` branch instead of the deny path.
#[allow(dead_code)]
pub fn lexicon_validate(nsid: &str, record: &serde_json::Value) -> Result<(), PdsError> {
    use proto_blue::lex_json::json_to_lex;
    use proto_blue::lexicon::{validate_record, LexUserType};

    let lexicons = kryphocron::lexicons();
    let def_uri = format!("{nsid}#main");
    let def = lexicons
        .get_def(&def_uri)
        .ok_or_else(|| PdsError::KryphocronLexiconMissing {
            def_uri: def_uri.clone(),
        })?;
    let lex_record = match def {
        LexUserType::Record(r) => r,
        _ => {
            return Err(PdsError::KryphocronLexiconNotRecord { def_uri });
        }
    };
    let lex_value = json_to_lex(record);
    validate_record(lexicons, lex_record, &lex_value).map_err(|e| {
        PdsError::KryphocronLexiconValidationFailed {
            nsid: nsid.to_string(),
            detail: e.to_string(),
        }
    })
}

/// Authorisation check for a kryphocron write under the `Ok(tier)`
/// dispatcher branch.
///
/// **Arc 1 stub.** Always returns `Ok(())`. The real implementation
/// in arc 2 consults a `WriteOp.kryphocron_authorization` field
/// (Q1 / Option A) populated by dedicated endpoints, cascade workers,
/// account-setup paths, and recovery-mode bypass. Arc 1's deny-by-
/// default rule makes this branch unreachable through the generic
/// write path, so the stub returning `Ok(())` is not exercised in
/// arc-1 ship state regardless of master-switch value.
#[allow(dead_code)]
pub fn check_authorization(
    _write: &crate::actor_store::repository::WriteOp,
) -> Result<(), PdsError> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Arc 2 — WriteOp authorization carrier (v07_DESIGN.md §5 lines 2140-2265)
// ---------------------------------------------------------------------------
//
// `KryphocronWriteAuthorization` is the per-write authorization flag set
// on `WriteOp.kryphocron_authorization` by the originating call path
// (dedicated endpoint handler, cascade worker, account-setup path,
// recovery-mode entry, or system-cleanup task). The dispatcher consults
// this flag at `validate_write` (arc 2 step 4) to permit a registered
// kryphocron NSID through the `Ok(tier)` branch — bypassing arc 1's
// deny-by-default rule under a specifically-authorized origin.
//
// Arc 2 step 2 defines the carrier types; step 3 builds the
// `CascadeContext` mint/verify machinery for `CascadeToken`; step 4
// wires the bind pipeline call at the check site. Steps 3/3.5/4/5/7
// consume these types.
//
// Translation note: v07_DESIGN.md uses `AtUri` for record identifiers
// in the cascade-source / system-cleanup variants. Aurora-Locus stores
// at-URIs as plain `String` throughout (`subject_uri` columns,
// `record.uri` PK, etc.); we follow the local idiom here rather than
// import `proto_blue::AtUri` just for the carrier types. Per
// `feedback_translate_spec_intent`, the spec intent — "carry the URI
// for audit/forensic context" — is preserved.

/// Per-write authorization flag set on `WriteOp.kryphocron_authorization`
/// by the originating call path. See v07_DESIGN.md §5 lines 2140-2193
/// for the full design rationale; arc 2 step 4 (`validate_write` bind
/// pipeline call) is the consumer.
///
/// Carries no `Clone` derive — `CascadeToken` is non-clonable by
/// design, so the enclosing enum cannot be `Clone` either. Carries
/// `#[serde(skip)]` on the WriteOp field so the wire shape of
/// `applyWrites` is unchanged from v0.6 (the authorization flag is
/// in-process state, not request-bearable input).
///
/// Arc 2 step 2 ships the enum shape with no production constructors;
/// steps 3 / 5 / 7 add constructors for `Cascade` (step 3
/// `CascadeContext::mint_token`) and `DedicatedEndpoint` (step 5
/// per-endpoint handlers). `AccountSetup` / `RecoveryBypass` /
/// `SystemCleanup` are post-arc-2 deferrals (recon R3 Path B for
/// `RecoveryBypass`; the others are scheduled for post-arc-2 cycles
/// once their originating call paths land). The `dead_code` allow
/// on the enum and its supporting types is intentional for the
/// step-2-only commit window.
#[derive(Debug)]
#[allow(dead_code)]
pub enum KryphocronWriteAuthorization {
    /// The write came through a dedicated `tools.aurora.kryphocron.*.*`
    /// endpoint (arc 2 step 5). Bind pipeline has been invoked (or
    /// skipped explicitly for Public-tier records) and authorized this
    /// write. `capability_class` matches the operation's class for
    /// audit-emit payload rendering.
    DedicatedEndpoint { capability_class: CapabilityClass },

    /// The write is a cascade from another authorized operation
    /// (e.g., bsky-side delete cascading to kryphocron companion,
    /// block-create cascading to audience updates). The cascade
    /// source is recorded for audit context. The token is minted by
    /// an active `CascadeContext` (arc 2 step 3) and verified at the
    /// check site (arc 2 step 4) to prevent cascade-authorization
    /// forgery.
    Cascade {
        source: CascadeSource,
        token: CascadeToken,
    },

    /// The write is an account-setup auto-create (default audience at
    /// account provisioning), backfill migration sweep, or
    /// lazy-create on first use. Permitted by the system-initiated
    /// path. The origin sub-variant distinguishes for audit.
    ///
    /// **R3-deferral notice.** Arc 2 ships the enum variant for
    /// forward-compat but no production code path constructs
    /// `AccountSetup` — the account-setup auto-create, backfill, and
    /// lazy-create paths are scheduled for a post-arc-2 cycle. The
    /// variant exists so the bind pipeline check site (arc 2 step 4)
    /// can match exhaustively against the design's authorization
    /// surface.
    #[allow(dead_code)]
    AccountSetup { origin: AccountSetupOrigin },

    /// The write is a recovery-mode bypass (`AURORA_RECOVERY_MODE=true`).
    /// The bind pipeline does NOT run; this variant exists to make
    /// the recovery-mode path observable in audit emit. When a
    /// cascade fires under recovery mode, the cascade WriteOp uses
    /// `RecoveryBypass` instead of `Cascade` (per v07_DESIGN.md §4
    /// "Composition surface, corrected").
    ///
    /// Unlike `Cascade`, `RecoveryBypass` carries no `CascadeToken`
    /// — the `cascade_source` field is informational only,
    /// identifying the originating cascade for forensic audit. There
    /// is no token-verification step at the check site. Verification
    /// is via the limited set of code paths that construct this
    /// variant (recovery-mode handler entry points and the
    /// recovery-mode cascade compositions per v07_DESIGN.md §4).
    ///
    /// **R3-deferral notice.** Arc 2 ships the enum variant for
    /// forward-compat but no production code path constructs
    /// `RecoveryBypass`. Recovery-mode write-path integration is
    /// deferred to a post-arc-2 cycle (recon R3 Path B); pre-v0.7
    /// recovery mode is a read-path single-key override only. The
    /// variant exists so the bind pipeline check site (arc 2 step 4)
    /// can match exhaustively against the design's authorization
    /// surface, and so the post-deferral cycle can wire constructors
    /// without re-shaping the carrier enum.
    #[allow(dead_code)]
    RecoveryBypass { cascade_source: Option<CascadeSource> },

    /// The write is a system-initiated automated cleanup running
    /// outside any user request and outside recovery mode. Bind
    /// pipeline does NOT run (the cleanup task already established
    /// authorization at the originating user action that produced
    /// the cleanup-eligible state — e.g., the user's bsky-side
    /// delete authorizes the eventual orphan-companion sweep; the
    /// user's `audience.delete` cascade failure authorizes the
    /// orphan-cascade revert). Each origin sub-variant identifies
    /// which cleanup task fired the write, for audit clarity.
    #[allow(dead_code)]
    SystemCleanup { origin: SystemCleanupOrigin },
}

/// Cascade source — identifies the originating cascade operation
/// that produced a `Cascade` or `RecoveryBypass` WriteOp.
///
/// Per v07_DESIGN.md §5 lines 2195-2200. URI fields carry the at-URI
/// of the originating record for audit-emit forensic context;
/// Aurora-Locus stores at-URIs as plain `String` (translation note
/// above).
///
/// The `Cascade` suffix on every variant matches the design's
/// naming (cross-doc consistency for forensic readers). Without
/// the suffix the variants collide semantically with other domain
/// concepts (e.g., `Block` vs. block-cascade); the clippy lint is
/// suppressed here for that reason.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code, clippy::enum_variant_names)]
pub enum CascadeSource {
    /// Bsky-side delete cascading to kryphocron companion delete (per
    /// v07_DESIGN.md §7e).
    BskyDeleteCascade { bsky_uri: String },
    /// Block-record create/delete cascading to per-audience updates
    /// (per v07_DESIGN.md §7c).
    BlockCascade { block_uri: String },
    /// Threadgate cascading from a `postPrivate` delete (per
    /// v07_DESIGN.md §7d), or as the depth-2 child of a
    /// `BskyDeleteCascade`.
    ThreadgateCascade { post_uri: String },
    /// Audience-delete cascading to per-post reassignment (per
    /// v07_DESIGN.md §7a).
    AudienceDeleteCascade { audience_uri: String },
}

/// Account-setup origin — identifies which system-initiated
/// account-setup path produced an `AccountSetup` WriteOp. Per
/// v07_DESIGN.md §5 lines 2202-2206.
///
/// Arc 2 ships the enum for forward-compat; no production code path
/// constructs these variants (R3-deferral parallel — account-setup
/// auto-create / backfill / lazy-create are post-arc-2 work).
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum AccountSetupOrigin {
    /// Default-audience auto-create at account provisioning.
    AccountSetup,
    /// Backfill migration sweep.
    Backfill,
    /// Lazy-create on first use.
    LazyCreate,
}

/// System-cleanup origin — identifies which cleanup task produced a
/// `SystemCleanup` WriteOp. Per v07_DESIGN.md §5 lines 2208-2253.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum SystemCleanupOrigin {
    /// Orphan-companion sweep — kryphocron-side companion delete for
    /// a `kryphocron_dual_link` row whose bsky_uri no longer
    /// resolves (per v07_DESIGN.md §7e). The backstop path.
    OrphanCompanionSweep { dual_link_id: i64 },
    /// Bsky-delete cascade completion — kryphocron-side companion
    /// delete via the immediate post-commit Tokio task (per
    /// v07_DESIGN.md §7e). The primary path.
    BskyDeleteCascadeCompletion {
        bsky_uri: String,
        dual_link_id: i64,
    },
    /// Orphan-cascade revert — restore a post's `audienceList` to the
    /// previous value after a failed audience-delete cascade (per
    /// v07_DESIGN.md §7a).
    OrphanCascadeRevert {
        cascade_id: Uuid,
        post_uri: String,
    },
}

/// Single-use, transactionally-scoped token authorizing a cascade
/// write. Minted by `CascadeContext::mint_token` (arc 2 step 3);
/// consumed by `validate_write` (arc 2 step 4).
///
/// Tokens are non-clonable, non-serializable, and bound to the
/// originating `CascadeContext` identity. Arc 2 step 3 builds the
/// mint/verify machinery; this step ships the opaque carrier so
/// `KryphocronWriteAuthorization::Cascade { .. }` is constructible
/// as a type but cannot be forged from outside the cascade-context
/// crate path.
///
/// Field layout: a `cascade_context_id` (process-local UUID minted at
/// `CascadeContext::new`) plus a `mint_id` (per-mint nonce). Arc 2
/// step 3 populates these via private constructors. Arc 2 step 2
/// (this commit) ships the type shape only; the field stays
/// `pub(crate)` so step 3 can wire mint/verify without API changes.
#[derive(Debug)]
pub struct CascadeToken {
    /// Identity of the issuing `CascadeContext` (arc 2 step 3).
    /// Populated by `CascadeContext::mint_token`; verified by
    /// `validate_write` against the active context.
    #[allow(dead_code)]
    pub(crate) cascade_context_id: Uuid,
    /// Per-mint nonce — distinguishes depth-1 mints from depth-2
    /// mints under the same context for the `mint_secondary_token`
    /// invariant (v07_DESIGN.md §5 depth-2 cap).
    #[allow(dead_code)]
    pub(crate) mint_id: Uuid,
}
