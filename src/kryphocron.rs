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

use serde::Serialize;
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
/// `HashMap<(nsid, op), DenyError>` populated from two sources:
///
/// - **Source 1** (per-endpoint override): for the NSIDs that now
///   have a dedicated XRPC procedure, the map points clients at
///   the dedicated endpoint via
///   `RequiresDedicatedEndpoint { suggested_endpoint: ... }`. The
///   four step-5 dedicated endpoints land entries here:
///   `tools.kryphocron.feed.postPrivate` (Create →
///   `createPostPrivate`; Delete → `deletePostPrivate`) and
///   `tools.kryphocron.policy.audience` (Create →
///   `manageAudience`). The `Update` action on `postPrivate` and
///   the `participatePrivate` endpoint's underlying record write
///   are left in source 2 (`NotYetSupported`) — there is no
///   dedicated edit-existing-private-post endpoint in arc 2,
///   and `participatePrivate` writes the same `postPrivate`
///   record collection so its create-path entry is the same
///   `createPostPrivate` suggestion.
///
/// - **Source 2** (registry-without-dedicated-endpoint): every
///   other `(NSID, action)` tuple gets `NotYetSupported`. The
///   registry walk runs first; source-1 overrides apply
///   afterward so the dedicated-endpoint entries replace the
///   defaults.
///
/// Both sources populate the same map at the same startup step.
/// Operators wanting to verify the surface can `cargo run --
/// list-kryphocron-nsids` once that CLI ships (post-arc-2), or
/// inspect the registry directly.
pub fn build_deny_map() -> HashMap<(String, WriteOpAction), KryphocronDenyVariant> {
    let mut map = HashMap::new();
    // Source 2 — default NotYetSupported for every registered
    // (NSID, action) tuple.
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
    // Source 1 — per-endpoint overrides. Applied AFTER the
    // registry walk so these entries replace the source-2
    // defaults for the NSIDs the four arc 2 step 5 dedicated
    // endpoints cover.
    map.insert(
        (
            crate::api::kryphocron_endpoints::NSID_POST_PRIVATE.to_string(),
            WriteOpAction::Create,
        ),
        KryphocronDenyVariant::RequiresDedicatedEndpoint {
            suggested_endpoint: Some(
                crate::api::kryphocron_endpoints::PROC_CREATE_POST_PRIVATE.to_string(),
            ),
        },
    );
    map.insert(
        (
            crate::api::kryphocron_endpoints::NSID_POST_PRIVATE.to_string(),
            WriteOpAction::Delete,
        ),
        KryphocronDenyVariant::RequiresDedicatedEndpoint {
            suggested_endpoint: Some(
                crate::api::kryphocron_endpoints::PROC_DELETE_POST_PRIVATE.to_string(),
            ),
        },
    );
    map.insert(
        (
            crate::api::kryphocron_endpoints::NSID_AUDIENCE.to_string(),
            WriteOpAction::Create,
        ),
        KryphocronDenyVariant::RequiresDedicatedEndpoint {
            suggested_endpoint: Some(
                crate::api::kryphocron_endpoints::PROC_MANAGE_AUDIENCE.to_string(),
            ),
        },
    );
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

// Arc 1's `check_authorization` stub was removed at arc 2 step 4.
// The dispatcher (in `RepositoryManager::validate_write`) now
// consults the `WriteOp.kryphocron_authorization` field directly
// and routes to [`bind_pipeline`] when it's `Some(_)`. The
// "authorization missing" semantic the kickoff named is the same
// as the existing deny-map / unregistered-NSID path: when auth is
// `None`, the dispatcher falls through to the arc-1 deny logic
// rather than calling a now-removed check function.

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
///
/// **Infallibility invariant (v0.8 arc 2 / #183).** Every variant
/// carries forensic identifiers only (at-URI `String`s), and every
/// variant MUST remain infallibly JSON-serializable — `serde_json::to_value`
/// must never return `Err` for any `CascadeSource`. The recovery-write
/// forensic-emit path in `bind_pipeline`'s `RecoveryBypass` arm serializes
/// `cascade_source` with `.expect("infallible")`; a future variant whose
/// `Serialize` impl could fail would panic in the audit-emit path rather
/// than silently corrupt the forensic record. The invariant is
/// structurally enforced by the compile-forced exhaustive test
/// `cascade_source_serialize_is_infallible_for_all_variants` (a new
/// variant without a match arm there fails to compile).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
/// `CascadeContext::new`) plus a `mint_id` (per-mint nonce). The
/// fields are `pub(crate)` so `CascadeContext::verify_token` can
/// read them across module boundaries within the crate while still
/// keeping the type unforgeable from outside the crate.
#[derive(Debug)]
pub struct CascadeToken {
    /// Identity of the issuing `CascadeContext`. Verified at the
    /// check site to enforce cross-context isolation — a token
    /// minted by context A cannot be verified by context B.
    pub(crate) cascade_context_id: Uuid,
    /// Per-mint nonce — registry key into `CascadeContext::mints`.
    /// Distinguishes depth-1 from depth-2 mints under the same
    /// context per v07_DESIGN.md §5 depth-2 cap; also enables the
    /// single-use spent-marker check on verify.
    pub(crate) mint_id: Uuid,
}

// ---------------------------------------------------------------------------
// Arc 2 step 3 — CascadeContext + token mint/verify
// ---------------------------------------------------------------------------
//
// `CascadeContext` is the producer side of `CascadeToken`. It tracks
// the cascade tree's root operation, the depth-2 one-shot invariant,
// and a per-mint registry the verify side consults at the check site.
// Per v07_DESIGN.md §5, the context's lifetime IS the transaction's
// lifetime — tokens minted during a tx that rolls back are voided
// implicitly because the context is dropped on tx end. Step 3 ships
// the type and its hostile tests; the transaction-lifetime semantics
// wire at step 3.5 (storage tx lending) and step 4 (validate_write
// bind pipeline call), where the context's drop point becomes
// observably tied to txn commit/rollback.

/// Single-context cascade-mint error returned by
/// [`CascadeContext::mint_secondary_token`].
///
/// Per v07_DESIGN.md §5 line 2293 ("preventing depth-3 chains"), each
/// `CascadeContext` may issue at most one depth-2 token. v0.7's
/// cascade inventory (BskyDeleteCascade → ThreadgateCascade is the
/// only depth-2-composing chain) is single-fanout per context, so a
/// one-shot depth-2 mint suffices; multi-fanout cascade trees (if a
/// future cycle introduces one) use one context per sub-tree rather
/// than relaxing this cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("kryphocron cascade depth exceeded — at most one depth-2 mint per context")]
pub struct KryphocronCascadeDepthExceeded;

/// Verify-side failure modes for [`CascadeContext::verify_token`].
///
/// Each variant corresponds to one of the four kickoff-hostile-test
/// failure cases:
/// - `ContextMismatch`: the token was minted by a different context
///   (hostile #4 — cross-context isolation).
/// - `UnknownToken`: the token's `mint_id` is not present in this
///   context's mint registry. Either a forged token, or a token
///   constructed via `#[derive(Default)]`-style code paths that
///   shouldn't exist (none do — CascadeToken has no Default impl).
/// - `SourceMismatch`: the supplied source does not match the source
///   recorded at mint time (hostile #5 — attacker swaps a token
///   minted for one cascade source onto a WriteOp claiming another).
/// - `AlreadySpent`: the token was previously verified successfully
///   and is now consumed (hostile #1 — single-use spent marker
///   prevents replay).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CascadeTokenError {
    /// Token was minted by a different `CascadeContext` (cross-
    /// context isolation breach attempt).
    #[error("cascade token minted by a different context")]
    ContextMismatch,
    /// Token's `mint_id` is not present in this context's mint
    /// registry — forged token or post-drop verify attempt.
    #[error("cascade token not present in mint registry (forged or stale)")]
    UnknownToken,
    /// Supplied source does not match the source the token was
    /// minted for.
    #[error("cascade token source does not match supplied source")]
    SourceMismatch,
    /// Token was previously verified; single-use replay rejected.
    #[error("cascade token already spent")]
    AlreadySpent,
}

/// Per-mint registry entry — stored under `CascadeContext::mints` by
/// `mint_id`. Records the cascade source the token was minted for
/// (used by verify-side source-mismatch check) and the one-shot
/// spent marker (flipped to true on successful verify).
#[derive(Debug)]
struct MintEntry {
    source: CascadeSource,
    /// 1 for depth-1 mints (from `mint_token`), 2 for depth-2
    /// mints (from `mint_secondary_token`). Recorded for forensic
    /// clarity at audit-emit time; the verify path does not gate
    /// on depth (any depth is verifiable as long as the other
    /// invariants hold).
    #[allow(dead_code)]
    depth: u8,
    spent: bool,
}

/// Cascade-tree state plus mint/verify machinery for the duration
/// of a single cascade-initiating transaction.
///
/// Construction marks the cascade-tree root: subsequent depth-1
/// mints (via [`mint_token`](Self::mint_token)) and the one-shot
/// depth-2 mint (via [`mint_secondary_token`](Self::mint_secondary_token))
/// reference this root for forensic correlation. Each mint registers
/// a unique `mint_id` keyed entry that the verify side
/// ([`verify_token`](Self::verify_token)) consumes on first
/// successful match. Single-use (spent-marker) is enforced
/// per-token.
///
/// **Transaction lifetime.** Per v07_DESIGN.md §5, the context's
/// lifetime IS the transaction's lifetime. Arc 2 step 3 ships the
/// type with no tx coupling; arc 2 step 3.5 wires the
/// `SqliteRepoStorage` lent-tx mechanism through which step 4
/// (`validate_write`) reaches the active context. Drop-on-rollback
/// happens implicitly because the context is owned by the
/// dispatcher frame that opens the txn.
///
/// **Thread safety.** All mutating methods take `&mut self`. The
/// context is single-owner per cascade dispatch — concurrent access
/// is not a use case the design supports (concurrent cascades use
/// distinct contexts).
#[derive(Debug)]
pub struct CascadeContext {
    id: Uuid,
    root_source: CascadeSource,
    mints: HashMap<Uuid, MintEntry>,
    /// One-shot flag for the depth-2 mint. Set true on first
    /// successful `mint_secondary_token`; subsequent attempts
    /// return `KryphocronCascadeDepthExceeded`. See the type-level
    /// rationale on [`KryphocronCascadeDepthExceeded`].
    secondary_minted: bool,
}

#[allow(dead_code)]
impl CascadeContext {
    /// Create a new context anchored at the given cascade-tree root
    /// source. `id` is a fresh process-local UUID; subsequent mints
    /// stamp their tokens with this id for the verify-side
    /// cross-context-isolation check.
    pub fn new(root_source: CascadeSource) -> Self {
        Self {
            id: Uuid::new_v4(),
            root_source,
            mints: HashMap::new(),
            secondary_minted: false,
        }
    }

    /// Stable process-local identifier for this context. Exposed
    /// for forensic correlation at audit-emit time.
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// The cascade-tree root operation this context was constructed
    /// for. Read-only after construction.
    pub fn root_source(&self) -> &CascadeSource {
        &self.root_source
    }

    /// Mint a depth-1 cascade token for `source`. Always succeeds
    /// (depth-1 fanout has no count cap — a single cascade-
    /// initiating operation may produce arbitrarily many depth-1
    /// children, e.g., one block-create may cascade to many
    /// audience updates).
    ///
    /// The minted token is registered in the context's per-mint
    /// registry. The verify side (`verify_token`) consults this
    /// registry on first verify; subsequent verifies of the same
    /// token error with `AlreadySpent`.
    pub fn mint_token(&mut self, source: CascadeSource) -> CascadeToken {
        let mint_id = Uuid::new_v4();
        self.mints.insert(
            mint_id,
            MintEntry {
                source,
                depth: 1,
                spent: false,
            },
        );
        CascadeToken {
            cascade_context_id: self.id,
            mint_id,
        }
    }

    /// Mint a depth-2 cascade token for `source`. One-shot per
    /// context: returns `KryphocronCascadeDepthExceeded` on the
    /// second call. See the type-level rationale on
    /// [`KryphocronCascadeDepthExceeded`] for why one-shot is
    /// sufficient for v0.7's cascade inventory.
    pub fn mint_secondary_token(
        &mut self,
        source: CascadeSource,
    ) -> Result<CascadeToken, KryphocronCascadeDepthExceeded> {
        if self.secondary_minted {
            return Err(KryphocronCascadeDepthExceeded);
        }
        self.secondary_minted = true;
        let mint_id = Uuid::new_v4();
        self.mints.insert(
            mint_id,
            MintEntry {
                source,
                depth: 2,
                spent: false,
            },
        );
        Ok(CascadeToken {
            cascade_context_id: self.id,
            mint_id,
        })
    }

    /// Verify and consume `token` against this context for the
    /// supplied `source`. Returns `Ok(())` iff:
    /// 1. `token.cascade_context_id == self.id` (cross-context
    ///    isolation — hostile #4)
    /// 2. `self.mints[token.mint_id]` exists (token is genuine)
    /// 3. `entry.source == source` (no source-swap forge —
    ///    hostile #5)
    /// 4. `!entry.spent` (single-use replay rejected — hostile #1)
    ///
    /// On success, the entry is marked spent; subsequent verifies
    /// of the same token return `AlreadySpent`.
    pub fn verify_token(
        &mut self,
        token: &CascadeToken,
        source: &CascadeSource,
    ) -> Result<(), CascadeTokenError> {
        if token.cascade_context_id != self.id {
            return Err(CascadeTokenError::ContextMismatch);
        }
        let entry = self
            .mints
            .get_mut(&token.mint_id)
            .ok_or(CascadeTokenError::UnknownToken)?;
        if &entry.source != source {
            return Err(CascadeTokenError::SourceMismatch);
        }
        if entry.spent {
            return Err(CascadeTokenError::AlreadySpent);
        }
        entry.spent = true;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Arc 2 step 4 — bind pipeline call
// ---------------------------------------------------------------------------
//
// `bind_pipeline` is the consumer side of the
// `KryphocronWriteAuthorization` carrier from step 2 and the
// `CascadeContext::verify_token` machinery from step 3. The
// dispatcher in `RepositoryManager::validate_write` routes
// `Some(_)` authorizations through this function; `None` falls
// through to arc 1's existing deny-by-default logic.
//
// Per v07_DESIGN.md §5/§6 the function fires one of three new
// tracing events on each call:
//
// - `kryphocron_bind_pipeline_authorized` — happy path
// - `kryphocron_bind_pipeline_denied` — reject for any reason
// - `kryphocron_cascade_token_invalid` — Cascade variant verify
//   failed (subset of `denied`)
//
// Arc 2 step 4 wires the framework; the actual oracle calls,
// stage-0/stage-5 predicates, audit-emit payloads, and timing
// equalization land at step 7 ("Audit emit shape for bind
// pipeline"). The five match arms here are the minimum surface
// step 5 ("Dedicated endpoints") needs to construct
// `DedicatedEndpoint`-variant WriteOps.

/// v0.7 arc 2 step 4 — execute the bind pipeline for a
/// kryphocron-authorized write.
///
/// **Audit emit lands on `shared_tx`.** Per the step-3.5 addendum's
/// audit-first relay-race ordering, all bind-pipeline audit
/// writes (step 7's payload-bearing inserts into
/// `moderation_event` + `mod_event_seq`) go to the shared
/// account-DB transaction the caller lent to `SqliteRepoStorage`.
/// Step 4 takes the `tx` parameter so step 7's wiring is a
/// trailing-additive change rather than a signature-break.
///
/// **CascadeContext** is consulted only by the `Cascade` arm.
/// When the variant is `Cascade { source, token }`, the function
/// expects `cascade_context: Some(_)` and calls
/// `verify_token(token, source)`. When `cascade_context: None` is
/// supplied with a `Cascade` variant, the function emits
/// `kryphocron_cascade_token_invalid` and rejects — there is no
/// active context, so the token cannot be verified.
///
/// **Arc 2 ship-state semantics.** No production code path
/// constructs `KryphocronWriteAuthorization::*` variants in arc 2
/// step 4. The first production constructor lands at step 5
/// (`DedicatedEndpoint` for the four user-class capabilities).
/// `Cascade`, `SystemCleanup`, `AccountSetup`, and `RecoveryBypass`
/// remain dead in production code at arc 2 ship state — the match
/// arms ship for forward compat and exhaustive coverage. The arc
/// 2 recon resolution supplement's R3 deferral applies to
/// `RecoveryBypass`; the deferral note on `AccountSetup` from
/// step 2 also applies here.
#[allow(dead_code)] // reached at arc 2 step 5 and onward
pub async fn bind_pipeline(
    write_op: &crate::actor_store::repository::WriteOp,
    shared_tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    cascade_context: Option<&mut CascadeContext>,
    did: &str,
    // v0.8 arc 1 (#180) — when an audit row is emitted onto
    // `shared_tx`, its `moderation_event.id` is pushed here so the
    // caller can persist an orphan marker if the paired actor commit
    // later fails. Empty when this write emits no audit row.
    emitted_event_ids: &mut Vec<i64>,
    // v0.8 arc 2 (#183) — when `validate_write` synthesizes a
    // `RecoveryBypass` under `AURORA_RECOVERY_MODE`, the auth can't be set
    // on the immutable, non-`Clone` `&WriteOp`, so it's threaded here.
    // `Some(_)` takes precedence over `write_op.kryphocron_authorization`
    // (M7); all non-recovery callers pass `None` (behavior-preserving).
    recovery_override: Option<KryphocronWriteAuthorization>,
) -> Result<(), PdsError> {
    // The `tx` parameter is reserved for step 7's audit-emit
    // inserts. Step 4 ships only the routing + tracing framework,
    // so the parameter is silenced here.
    let _ = &shared_tx;

    // v0.8 arc 2 (#183) — `recovery_override` takes precedence (M7). When
    // present it IS the auth (a synthesized `RecoveryBypass`); otherwise read
    // the write's own authorization. `KryphocronWriteAuthorization` is
    // `Debug`-only (non-`Clone`), so both arms yield a borrow via `.as_ref()`.
    let auth = match recovery_override.as_ref() {
        Some(a) => a,
        None => write_op
            .kryphocron_authorization
            .as_ref()
            .ok_or_else(|| {
                // Programmer error: the dispatcher only calls bind_pipeline
                // when authorization is Some(_) or a recovery_override is set.
                tracing::warn!(
                    target: "aurora_locus::kryphocron",
                    event = "kryphocron_bind_pipeline_denied",
                    did = %did,
                    nsid = %write_op.collection,
                    reason = "no_authorization",
                );
                PdsError::Internal(
                    "bind_pipeline invoked without kryphocron_authorization".to_string(),
                )
            })?,
    };

    match auth {
        KryphocronWriteAuthorization::DedicatedEndpoint { capability_class } => {
            tracing::info!(
                target: "aurora_locus::kryphocron",
                event = "kryphocron_bind_pipeline_authorized",
                did = %did,
                nsid = %write_op.collection,
                variant = "DedicatedEndpoint",
                capability_class = ?capability_class,
            );

            // v0.7 arc 2 step 7 — housekeeping audit emit per
            // v07_DESIGN.md §4 category B. The lent shared tx
            // committed by `commit_with_orphan_recovery` (step
            // 3.5) commits BEFORE the actor tx carrying the
            // record write, so this row commits transactionally
            // with the record write — the audit-coherence
            // design's "transactional with record write" property
            // now actually holds end-to-end (not best-effort).
            //
            // Per the §4 "Where housekeeping events fire" table,
            // the audience-list dedicated endpoint
            // (`manageAudience`) emits
            // `KryphocronAudienceUpdated`. The block / mute /
            // threadgate dedicated endpoints are post-arc-2 work
            // — their B variants ship as enum + payload only
            // (see `kryphocron_audit` module rustdoc).
            //
            // Per-class bind-pipeline stages (oracle consultation,
            // stage-0/stage-5, timing equalization) are
            // substrate-integration work scheduled for cycles
            // beyond arc 2. The substrate's bind pipeline is the
            // authoritative source for the per-class stages;
            // Aurora-Locus's job is the audit emit + the host-
            // side audience check (wired below for
            // ParticipatePrivate). For `DedicatedEndpoint`-
            // authorized writes that DON'T hit the audience
            // collection, the bind succeeds with tracing-only
            // until later cycles wire per-class stages.
            if write_op.collection == "tools.kryphocron.policy.audience" {
                let audience_uri = format!(
                    "at://{}/{}/{}",
                    did, write_op.collection, write_op.rkey
                );
                let operation = match write_op.action {
                    crate::actor_store::repository::WriteOpAction::Create => {
                        crate::kryphocron_audit::AudienceOperation::Created
                    }
                    crate::actor_store::repository::WriteOpAction::Update => {
                        crate::kryphocron_audit::AudienceOperation::Updated
                    }
                    crate::actor_store::repository::WriteOpAction::Delete => {
                        crate::kryphocron_audit::AudienceOperation::Deleted
                    }
                };

                // Arc 2 step 7 ships a minimal payload — the
                // mode / member-diff / cascade-field extraction
                // requires walking the record body and the
                // resume-state table. Post-arc-2 cycles can
                // tighten this without re-shaping the event
                // surface (the payload struct's shape is the
                // design's §4 shape). Members are emitted as the
                // raw `did:plc:...` list from the record body if
                // present; mode is read from the record's
                // top-level `mode` field if present, defaulting
                // to `"list"`.
                let value = write_op.value.as_ref();
                let mode_after = value
                    .and_then(|v| v.get("mode"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("list")
                    .to_string();
                let members_added = value
                    .and_then(|v| v.get("members"))
                    .and_then(|m| m.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let members_total_after = members_added.len() as i64;
                let name = value
                    .and_then(|v| v.get("name"))
                    .and_then(|m| m.as_str())
                    .map(String::from);

                let payload = crate::kryphocron_audit::AudienceUpdatedPayload {
                    audience_uri,
                    owner_did: did.to_string(),
                    operation,
                    members_added,
                    members_removed: vec![],
                    members_total_after,
                    mode_before: None,
                    mode_after,
                    name,
                    origin: crate::kryphocron_audit::AudienceOrigin::User,
                    cascade_id: None,
                    cascade_reassigned_to: None,
                    cascade_post_count: None,
                    cascade_progress: None,
                };

                // v0.8 arc 1 (#180) — capture the new
                // moderation_event.id so the relay-race caller can
                // persist a `bind_audit_orphan_marker` row keyed off
                // it if the paired actor commit fails after this audit
                // row commits on `shared_tx`.
                let event_id = crate::kryphocron_audit::emit_audience_updated_in_tx(
                    shared_tx,
                    did,
                    payload,
                )
                .await?;
                emitted_event_ids.push(event_id);
            }

            Ok(())
        }
        KryphocronWriteAuthorization::Cascade { source, token } => {
            let ctx = match cascade_context {
                Some(c) => c,
                None => {
                    tracing::warn!(
                        target: "aurora_locus::kryphocron",
                        event = "kryphocron_cascade_token_invalid",
                        did = %did,
                        nsid = %write_op.collection,
                        reason = "no_active_cascade_context",
                    );
                    tracing::warn!(
                        target: "aurora_locus::kryphocron",
                        event = "kryphocron_bind_pipeline_denied",
                        did = %did,
                        nsid = %write_op.collection,
                        variant = "Cascade",
                        reason = "no_active_cascade_context",
                    );
                    return Err(PdsError::KryphocronCascadeTokenInvalid(
                        "no active CascadeContext at bind_pipeline call site".to_string(),
                    ));
                }
            };
            match ctx.verify_token(token, source) {
                Ok(()) => {
                    tracing::info!(
                        target: "aurora_locus::kryphocron",
                        event = "kryphocron_bind_pipeline_authorized",
                        did = %did,
                        nsid = %write_op.collection,
                        variant = "Cascade",
                        source = ?source,
                    );
                    // TODO step 7: per-source cascade bind stages
                    Ok(())
                }
                Err(e) => {
                    tracing::warn!(
                        target: "aurora_locus::kryphocron",
                        event = "kryphocron_cascade_token_invalid",
                        did = %did,
                        nsid = %write_op.collection,
                        verify_error = %e,
                    );
                    tracing::warn!(
                        target: "aurora_locus::kryphocron",
                        event = "kryphocron_bind_pipeline_denied",
                        did = %did,
                        nsid = %write_op.collection,
                        variant = "Cascade",
                        verify_error = %e,
                    );
                    Err(PdsError::KryphocronCascadeTokenInvalid(e.to_string()))
                }
            }
        }
        KryphocronWriteAuthorization::AccountSetup { origin } => {
            tracing::info!(
                target: "aurora_locus::kryphocron",
                event = "kryphocron_bind_pipeline_authorized",
                did = %did,
                nsid = %write_op.collection,
                variant = "AccountSetup",
                origin = ?origin,
            );
            // Account-setup writes don't run the bind pipeline
            // proper (no oracle consultation for system-initiated
            // auto-creates); per the design they emit an
            // AccountSetup audit event only. Step 4 ships the
            // routing + tracing; step 7 wires the audit emit.
            // R3-deferred per the supplement — no production
            // constructor exists in arc 2.
            Ok(())
        }
        KryphocronWriteAuthorization::RecoveryBypass { cascade_source } => {
            tracing::info!(
                target: "aurora_locus::kryphocron",
                event = "kryphocron_bind_pipeline_authorized",
                did = %did,
                nsid = %write_op.collection,
                variant = "RecoveryBypass",
                cascade_source = ?cascade_source,
            );
            // v0.8 arc 2 (#183) — emit a persistent `KryphocronRecoveryWrite`
            // row on the lent `shared_tx` (M1 audit-first ordering) and push
            // its `moderation_event.id` into `emitted_event_ids` so a paired
            // actor-commit failure is swept by the arc 1 orphan reconcile
            // (M2 participation). The bind pipeline proper does NOT run —
            // recovery mode trusts the operator (Q5 full bypass); the emit IS
            // the forensic record. Production construction is env-gated in
            // `validate_write` under `AURORA_RECOVERY_MODE` (Q3).
            let action = match &write_op.action {
                WriteOpAction::Create => "create",
                WriteOpAction::Update => "update",
                WriteOpAction::Delete => "delete",
            }
            .to_string();
            // cascade_source bridge with an infallibility guard (M8): `.expect`
            // makes a future fallible-`Serialize` `CascadeSource` variant PANIC
            // here rather than silently drop forensic data via `.ok()`. Always
            // `None` in arc 2 ship state; the `Some` branch is forward-compat.
            let cascade_source_json = cascade_source.as_ref().map(|cs| {
                serde_json::to_value(cs).expect(
                    "CascadeSource must be infallibly JSON-serializable; if you \
                     added a variant with a fallible Serialize, fix it before \
                     populating cascade_source. See the CascadeSource rustdoc invariant.",
                )
            });
            let payload = crate::kryphocron_audit::RecoveryWritePayload {
                subject_uri: format!(
                    "at://{}/{}/{}",
                    did, write_op.collection, write_op.rkey
                ),
                requester_did: did.to_string(),
                nsid: write_op.collection.clone(),
                action,
                cascade_source: cascade_source_json,
            };
            let event_id = crate::kryphocron_audit::emit_recovery_write_in_tx(
                shared_tx, did, &payload,
            )
            .await?;
            emitted_event_ids.push(event_id);
            Ok(())
        }
        KryphocronWriteAuthorization::SystemCleanup { origin } => {
            tracing::info!(
                target: "aurora_locus::kryphocron",
                event = "kryphocron_bind_pipeline_authorized",
                did = %did,
                nsid = %write_op.collection,
                variant = "SystemCleanup",
                origin = ?origin,
            );
            // System-cleanup writes (orphan-companion sweep,
            // bsky-delete cascade completion, orphan-cascade
            // revert) don't run the bind pipeline — authorization
            // was established at the originating user action.
            // Audit emit happens at step 7. No production
            // constructor in arc 2.
            Ok(())
        }
    }
}

#[cfg(test)]
mod cascade_context_tests {
    use super::*;

    fn bsky_root() -> CascadeSource {
        CascadeSource::BskyDeleteCascade {
            bsky_uri: "at://did:plc:test/app.bsky.feed.post/abc".to_string(),
        }
    }

    fn block_source() -> CascadeSource {
        CascadeSource::BlockCascade {
            block_uri: "at://did:plc:test/app.bsky.graph.block/xyz".to_string(),
        }
    }

    /// Hostile #1 — mint depth-1, verify ok, second verify fails
    /// with `AlreadySpent`. Single-use replay protection.
    #[test]
    fn hostile_1_single_use_spent_marker_blocks_replay() {
        let mut ctx = CascadeContext::new(bsky_root());
        let source = bsky_root();
        let token = ctx.mint_token(source.clone());

        assert!(ctx.verify_token(&token, &source).is_ok(), "first verify");
        assert_eq!(
            ctx.verify_token(&token, &source),
            Err(CascadeTokenError::AlreadySpent),
            "second verify must fail with AlreadySpent",
        );
    }

    /// Hostile #2 — mint depth-1 and depth-2 in the same context,
    /// both verify successfully.
    #[test]
    fn hostile_2_depth_1_and_depth_2_both_verify() {
        let mut ctx = CascadeContext::new(bsky_root());
        let source = bsky_root();

        let primary = ctx.mint_token(source.clone());
        let secondary = ctx
            .mint_secondary_token(source.clone())
            .expect("first secondary mint must succeed");

        assert!(
            ctx.verify_token(&primary, &source).is_ok(),
            "depth-1 token verifies",
        );
        assert!(
            ctx.verify_token(&secondary, &source).is_ok(),
            "depth-2 token verifies",
        );
    }

    /// Hostile #3 — after one successful depth-2 mint, a second
    /// `mint_secondary_token` call returns
    /// `KryphocronCascadeDepthExceeded`. The one-shot-per-context
    /// rule per v07_DESIGN.md §5 line 2293.
    #[test]
    fn hostile_3_second_secondary_mint_returns_depth_exceeded() {
        let mut ctx = CascadeContext::new(bsky_root());
        let source = bsky_root();

        let _first = ctx
            .mint_secondary_token(source.clone())
            .expect("first secondary mint must succeed");

        let second = ctx.mint_secondary_token(source);
        assert_eq!(
            second.unwrap_err(),
            KryphocronCascadeDepthExceeded,
            "second secondary mint must fail with depth-exceeded",
        );
    }

    /// Hostile #4 — a token minted by context A cannot be verified
    /// by context B. Cross-context isolation: each context's
    /// process-local UUID gates verify.
    #[test]
    fn hostile_4_cross_context_verify_rejected() {
        let mut ctx_a = CascadeContext::new(bsky_root());
        let mut ctx_b = CascadeContext::new(bsky_root());
        let source = bsky_root();

        let token_from_a = ctx_a.mint_token(source.clone());
        assert_eq!(
            ctx_b.verify_token(&token_from_a, &source),
            Err(CascadeTokenError::ContextMismatch),
            "ctx_b must reject token minted by ctx_a",
        );
    }

    /// Hostile #5 — a token minted for `BskyDeleteCascade` does
    /// not verify against `BlockCascade`. Source-mismatch forge
    /// detection: an attacker who swaps the source on a Cascade
    /// WriteOp cannot reuse a legitimate token for a different
    /// cascade type.
    #[test]
    fn hostile_5_source_mismatch_rejected() {
        let mut ctx = CascadeContext::new(bsky_root());
        let mint_source = bsky_root();
        let attack_source = block_source();

        let token = ctx.mint_token(mint_source);
        assert_eq!(
            ctx.verify_token(&token, &attack_source),
            Err(CascadeTokenError::SourceMismatch),
            "verify with mismatched source must fail",
        );
    }

    /// Defense-in-depth — a forged token (random UUIDs not minted
    /// by any context) is rejected with `UnknownToken` when
    /// presented to a context whose id happens to match. Covers
    /// the `mints` HashMap lookup miss path in `verify_token`.
    #[test]
    fn forged_token_with_matching_context_id_rejected_as_unknown() {
        let mut ctx = CascadeContext::new(bsky_root());
        let source = bsky_root();

        let forged = CascadeToken {
            cascade_context_id: ctx.id(),
            mint_id: Uuid::new_v4(),
        };
        assert_eq!(
            ctx.verify_token(&forged, &source),
            Err(CascadeTokenError::UnknownToken),
            "forged token must be rejected even with matching ctx id",
        );
    }

    /// Sanity — context exposes its root source via the public
    /// accessor; the field is stable across mints.
    #[test]
    fn root_source_accessor_stable_across_mints() {
        let root = bsky_root();
        let mut ctx = CascadeContext::new(root.clone());
        assert_eq!(ctx.root_source(), &root, "before mint");

        let _ = ctx.mint_token(root.clone());
        assert_eq!(ctx.root_source(), &root, "after depth-1 mint");

        let _ = ctx.mint_secondary_token(root.clone());
        assert_eq!(ctx.root_source(), &root, "after depth-2 mint");
    }
}

#[cfg(test)]
mod bind_pipeline_tests {
    //! v0.7 arc 2 step 4 — bind_pipeline unit tests.
    //!
    //! Coverage:
    //!  - DedicatedEndpoint arm fires `kryphocron_bind_pipeline_authorized`
    //!  - Cascade arm with valid token → `authorized`; with invalid
    //!    token → `cascade_token_invalid` + `denied`; with no
    //!    context → `cascade_token_invalid` + `denied`
    //!  - AccountSetup, RecoveryBypass, SystemCleanup arms fire
    //!    `authorized` (R3-deferred — no production constructor;
    //!    arm exists for exhaustive match coverage)

    use super::*;
    use crate::actor_store::repository::{WriteOp, WriteOpAction};
    use sqlx::AnyPool;
    use std::sync::atomic::{AtomicU64, Ordering};

    async fn fresh_shared_pool() -> AnyPool {
        sqlx::any::install_default_drivers();
        AnyPool::connect("sqlite::memory:")
            .await
            .expect("shared pool")
    }

    /// Shared pool with the `moderation_event` table (migration 0001
    /// columns) so the `RecoveryBypass` arm's `emit_recovery_write_in_tx`
    /// INSERT can land. Mirrors the manual-CREATE pattern Arc 1's
    /// `step_3_5_t8` uses for the bind-audit tables. Pinned to a single
    /// connection: each `sqlite::memory:` connection is its own database, so
    /// the CREATE TABLE and the later `begin()` must share one connection.
    async fn fresh_shared_pool_with_moderation_event() -> AnyPool {
        sqlx::any::install_default_drivers();
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("shared pool");
        sqlx::query(
            "CREATE TABLE moderation_event (\
                 id INTEGER PRIMARY KEY AUTOINCREMENT, \
                 event_type TEXT NOT NULL, \
                 actor_did TEXT NOT NULL, \
                 subject_did TEXT, \
                 subject_uri TEXT, \
                 subject_cid TEXT, \
                 details TEXT NOT NULL, \
                 created_at TEXT NOT NULL, \
                 meta TEXT)",
        )
        .execute(&pool)
        .await
        .expect("create moderation_event");
        // insert_moderation_event_in_tx dual-writes mod_event_seq (0006).
        sqlx::query(
            "CREATE TABLE mod_event_seq (\
                 seq INTEGER PRIMARY KEY AUTOINCREMENT, \
                 moderation_event_id INTEGER NOT NULL, \
                 actor_did TEXT NOT NULL, \
                 action TEXT NOT NULL, \
                 subject_did TEXT, \
                 subject_uri TEXT, \
                 subject_cid TEXT, \
                 detail TEXT, \
                 created_at TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .expect("create mod_event_seq");
        pool
    }

    /// Build a `WriteOp` carrying an explicit `WriteOpAction` (the
    /// `make_write` helper hardcodes `Create`; the action-mapping test
    /// needs Update/Delete too).
    fn make_write_with_action(
        nsid: &str,
        action: WriteOpAction,
        auth: KryphocronWriteAuthorization,
    ) -> WriteOp {
        let mut w = make_write(nsid, auth);
        w.action = action;
        w
    }

    fn make_write(nsid: &str, auth: KryphocronWriteAuthorization) -> WriteOp {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        WriteOp {
            action: WriteOpAction::Create,
            collection: nsid.to_string(),
            rkey: format!("bp{}", n),
            value: Some(serde_json::json!({"$type": nsid})),
            validate: None,
            swap_cid: None,
            kryphocron_authorization: Some(auth),
        }
    }

    fn bsky_source() -> CascadeSource {
        CascadeSource::BskyDeleteCascade {
            bsky_uri: "at://did:plc:bp/app.bsky.feed.post/abc".to_string(),
        }
    }

    fn block_source() -> CascadeSource {
        CascadeSource::BlockCascade {
            block_uri: "at://did:plc:bp/app.bsky.graph.block/xyz".to_string(),
        }
    }

    /// DedicatedEndpoint arm → authorized event, returns Ok.
    #[tokio::test(flavor = "multi_thread")]
    #[tracing_test::traced_test]
    async fn dedicated_endpoint_emits_authorized() {
        let pool = fresh_shared_pool().await;
        let mut tx = pool.begin().await.expect("begin shared tx");
        let write = make_write(
            "tools.kryphocron.feed.postPrivate",
            KryphocronWriteAuthorization::DedicatedEndpoint {
                capability_class: CapabilityClass::User,
            },
        );

        bind_pipeline(&write, &mut tx, None, "did:plc:bp1", &mut Vec::new(), None)
            .await
            .expect("DedicatedEndpoint arm must succeed");

        assert!(
            logs_contain("kryphocron_bind_pipeline_authorized"),
            "authorized event must fire",
        );
        assert!(
            logs_contain("DedicatedEndpoint"),
            "variant tag must surface in the event",
        );
        assert!(
            !logs_contain("kryphocron_bind_pipeline_denied"),
            "no denied event on the happy path",
        );
    }

    /// Cascade arm with a valid token + matching CascadeContext →
    /// authorized event, returns Ok, token marked spent.
    #[tokio::test(flavor = "multi_thread")]
    #[tracing_test::traced_test]
    async fn cascade_with_valid_token_emits_authorized() {
        let pool = fresh_shared_pool().await;
        let mut tx = pool.begin().await.expect("begin shared tx");
        let mut ctx = CascadeContext::new(bsky_source());
        let token = ctx.mint_token(bsky_source());

        let write = make_write(
            "tools.kryphocron.feed.postPrivate",
            KryphocronWriteAuthorization::Cascade {
                source: bsky_source(),
                token,
            },
        );

        bind_pipeline(&write, &mut tx, Some(&mut ctx), "did:plc:bp2", &mut Vec::new(), None)
            .await
            .expect("Cascade with valid token must succeed");

        assert!(
            logs_contain("kryphocron_bind_pipeline_authorized"),
            "authorized event must fire",
        );
        assert!(
            !logs_contain("kryphocron_cascade_token_invalid"),
            "no invalid event for the valid-token path",
        );
    }

    /// Cascade arm with NO active CascadeContext → cascade_token_invalid
    /// + denied events, returns KryphocronCascadeTokenInvalid.
    #[tokio::test(flavor = "multi_thread")]
    #[tracing_test::traced_test]
    async fn cascade_without_context_rejected() {
        let pool = fresh_shared_pool().await;
        let mut tx = pool.begin().await.expect("begin shared tx");

        // Construct a Cascade WriteOp with a synthetic token —
        // there's no CascadeContext to mint it from, so the token
        // points to a context that won't be supplied at verify
        // time. The bind_pipeline must reject regardless of token
        // validity because no context is available to verify
        // against.
        let dummy_ctx = CascadeContext::new(bsky_source());
        let synthetic_token = CascadeToken {
            cascade_context_id: dummy_ctx.id(),
            mint_id: uuid::Uuid::new_v4(),
        };
        let write = make_write(
            "tools.kryphocron.feed.postPrivate",
            KryphocronWriteAuthorization::Cascade {
                source: bsky_source(),
                token: synthetic_token,
            },
        );

        let result = bind_pipeline(&write, &mut tx, None, "did:plc:bp3", &mut Vec::new(), None).await;
        assert!(
            matches!(result, Err(PdsError::KryphocronCascadeTokenInvalid(_))),
            "must reject Cascade with no active context: {result:?}",
        );
        assert!(
            logs_contain("kryphocron_cascade_token_invalid"),
            "invalid event must fire",
        );
        assert!(
            logs_contain("kryphocron_bind_pipeline_denied"),
            "denied event must fire alongside invalid",
        );
        assert!(
            logs_contain("no_active_cascade_context"),
            "reason tag must surface",
        );
    }

    /// Cascade arm with a token from a DIFFERENT context (cross-
    /// context isolation, same as step-3 hostile #4) → invalid +
    /// denied, rejects.
    #[tokio::test(flavor = "multi_thread")]
    #[tracing_test::traced_test]
    async fn cascade_with_cross_context_token_rejected() {
        let pool = fresh_shared_pool().await;
        let mut tx = pool.begin().await.expect("begin shared tx");

        // Mint a token from ctx_a, supply ctx_b at verify time.
        let mut ctx_a = CascadeContext::new(bsky_source());
        let mut ctx_b = CascadeContext::new(bsky_source());
        let token_from_a = ctx_a.mint_token(bsky_source());

        let write = make_write(
            "tools.kryphocron.feed.postPrivate",
            KryphocronWriteAuthorization::Cascade {
                source: bsky_source(),
                token: token_from_a,
            },
        );

        let result = bind_pipeline(&write, &mut tx, Some(&mut ctx_b), "did:plc:bp4", &mut Vec::new(), None).await;
        assert!(
            matches!(result, Err(PdsError::KryphocronCascadeTokenInvalid(_))),
            "ctx_b must reject a token minted by ctx_a: {result:?}",
        );
        assert!(
            logs_contain("kryphocron_cascade_token_invalid"),
            "invalid event must fire on context mismatch",
        );
    }

    /// Cascade arm with a source-mismatched token (mint with
    /// BskyDeleteCascade, present BlockCascade — same as step-3
    /// hostile #5) → invalid + denied, rejects.
    #[tokio::test(flavor = "multi_thread")]
    #[tracing_test::traced_test]
    async fn cascade_with_source_mismatched_token_rejected() {
        let pool = fresh_shared_pool().await;
        let mut tx = pool.begin().await.expect("begin shared tx");
        let mut ctx = CascadeContext::new(bsky_source());
        let token = ctx.mint_token(bsky_source());

        // Present the token with a DIFFERENT source.
        let write = make_write(
            "tools.kryphocron.feed.postPrivate",
            KryphocronWriteAuthorization::Cascade {
                source: block_source(),
                token,
            },
        );

        let result = bind_pipeline(&write, &mut tx, Some(&mut ctx), "did:plc:bp5", &mut Vec::new(), None).await;
        assert!(
            matches!(result, Err(PdsError::KryphocronCascadeTokenInvalid(_))),
            "source mismatch must reject: {result:?}",
        );
        assert!(
            logs_contain("kryphocron_cascade_token_invalid"),
            "invalid event must fire on source mismatch",
        );
    }

    /// AccountSetup arm → authorized event, returns Ok. R3-deferred
    /// (no production constructor) but the arm is reachable in
    /// tests; coverage proves the match arm exists and the
    /// tracing event names the variant.
    #[tokio::test(flavor = "multi_thread")]
    #[tracing_test::traced_test]
    async fn account_setup_emits_authorized() {
        let pool = fresh_shared_pool().await;
        let mut tx = pool.begin().await.expect("begin shared tx");
        let write = make_write(
            "tools.kryphocron.audience.list",
            KryphocronWriteAuthorization::AccountSetup {
                origin: AccountSetupOrigin::AccountSetup,
            },
        );

        bind_pipeline(&write, &mut tx, None, "did:plc:bp6", &mut Vec::new(), None)
            .await
            .expect("AccountSetup arm must succeed");

        assert!(logs_contain("kryphocron_bind_pipeline_authorized"));
        assert!(logs_contain("AccountSetup"));
    }

    /// RecoveryBypass arm → authorized event, returns Ok. R3-
    /// deferred per arc 2 supplement; arm exists for exhaustive
    /// coverage of the design's authorization surface.
    ///
    /// v0.8 arc 2 (#183, §6.1 + §6.9) — extended from "asserts tracing only"
    /// to also assert the persistent `kryphocron_recovery_write`
    /// `moderation_event` row lands AND the event id is pushed into
    /// `emitted_event_ids`. The push assertion is the M2 wiring tripwire: a
    /// row could land via the INSERT without the push happening, which would
    /// make the recovery write unsweepable by the arc 1 orphan reconcile.
    #[tokio::test(flavor = "multi_thread")]
    #[tracing_test::traced_test]
    async fn recovery_bypass_emits_authorized() {
        use sqlx::Row as _;
        let pool = fresh_shared_pool_with_moderation_event().await;
        let mut tx = pool.begin().await.expect("begin shared tx");
        let write = make_write(
            "tools.kryphocron.feed.postPrivate",
            KryphocronWriteAuthorization::RecoveryBypass { cascade_source: None },
        );
        let mut emitted: Vec<i64> = Vec::new();

        bind_pipeline(&write, &mut tx, None, "did:plc:bp7", &mut emitted, None)
            .await
            .expect("RecoveryBypass arm must succeed");

        assert!(logs_contain("kryphocron_bind_pipeline_authorized"));
        assert!(logs_contain("RecoveryBypass"));

        // Persistent forensic row landed on the lent tx (M1).
        let row = sqlx::query(
            "SELECT id, event_type, actor_did, subject_uri FROM moderation_event",
        )
        .fetch_one(&mut *tx)
        .await
        .expect("exactly one moderation_event row");
        let event_id: i64 = row.try_get("id").expect("id");
        let event_type: String = row.try_get("event_type").expect("event_type");
        let actor_did: String = row.try_get("actor_did").expect("actor_did");
        let subject_uri: String = row.try_get("subject_uri").expect("subject_uri");
        assert_eq!(event_type, "kryphocron_recovery_write");
        assert_eq!(actor_did, "did:plc:bp7");
        assert_eq!(
            subject_uri,
            format!("at://did:plc:bp7/tools.kryphocron.feed.postPrivate/{}", write.rkey),
            "subject_uri = at://<did>/<collection>/<rkey>; non-NULL (cross-arc orphan invariant)",
        );

        // M2 tripwire — the event id was pushed, not just inserted.
        assert_eq!(emitted, vec![event_id], "M2: emitted_event_ids must hold the row id");
    }

    /// v0.8 arc 2 (#183, §6.2) — a `RecoveryBypass` carrying a
    /// `cascade_source` serializes it into the persisted payload's JSON
    /// `cascade_source` field. Exercises the `CascadeSource` `Serialize`
    /// derive end-to-end through the arm. (Forward-compat: production only
    /// ever constructs `cascade_source: None` in arc 2, M8.)
    #[tokio::test(flavor = "multi_thread")]
    async fn recovery_bypass_serializes_cascade_source() {
        use sqlx::Row as _;
        let pool = fresh_shared_pool_with_moderation_event().await;
        let mut tx = pool.begin().await.expect("begin shared tx");
        let write = make_write(
            "tools.kryphocron.feed.postPrivate",
            KryphocronWriteAuthorization::RecoveryBypass {
                cascade_source: Some(CascadeSource::AudienceDeleteCascade {
                    audience_uri: "at://did:plc:x/tools.kryphocron.policy.audience/aud".to_string(),
                }),
            },
        );
        let mut emitted: Vec<i64> = Vec::new();

        bind_pipeline(&write, &mut tx, None, "did:plc:cs", &mut emitted, None)
            .await
            .expect("RecoveryBypass arm must succeed");

        let details: String =
            sqlx::query("SELECT details FROM moderation_event")
                .fetch_one(&mut *tx)
                .await
                .expect("row")
                .try_get("details")
                .expect("details");
        let parsed: serde_json::Value =
            serde_json::from_str(&details).expect("details is valid JSON");
        let expected = serde_json::to_value(CascadeSource::AudienceDeleteCascade {
            audience_uri: "at://did:plc:x/tools.kryphocron.policy.audience/aud".to_string(),
        })
        .unwrap();
        assert_eq!(parsed["cascade_source"], expected);
    }

    /// v0.8 arc 2 (#183, §6.3) — `WriteOpAction::{Create, Update, Delete}`
    /// maps to payload `action` `"create" / "update" / "delete"`.
    #[tokio::test(flavor = "multi_thread")]
    async fn recovery_bypass_action_mapping() {
        use sqlx::Row as _;
        for (op, expected) in [
            (WriteOpAction::Create, "create"),
            (WriteOpAction::Update, "update"),
            (WriteOpAction::Delete, "delete"),
        ] {
            let pool = fresh_shared_pool_with_moderation_event().await;
            let mut tx = pool.begin().await.expect("begin shared tx");
            let write = make_write_with_action(
                "tools.kryphocron.feed.postPrivate",
                op,
                KryphocronWriteAuthorization::RecoveryBypass { cascade_source: None },
            );
            let mut emitted: Vec<i64> = Vec::new();
            bind_pipeline(&write, &mut tx, None, "did:plc:act", &mut emitted, None)
                .await
                .expect("RecoveryBypass arm must succeed");
            let details: String = sqlx::query("SELECT details FROM moderation_event")
                .fetch_one(&mut *tx)
                .await
                .expect("row")
                .try_get("details")
                .expect("details");
            let parsed: serde_json::Value =
                serde_json::from_str(&details).expect("valid JSON");
            assert_eq!(parsed["action"], expected, "action map for {op:?}");
        }
    }

    /// v0.8 arc 2 (#183, §6.7 / LB3 layer 2) — every shipping `CascadeSource`
    /// variant is infallibly JSON-serializable, AND the exhaustive `match`
    /// (no `_` arm) compile-forces a future variant into this coverage: a new
    /// `CascadeSource` variant without an arm here fails to compile, which is
    /// the structural enforcement of the §3.1 rustdoc infallibility invariant
    /// the arm's `.expect()` relies on.
    #[test]
    fn cascade_source_serialize_is_infallible_for_all_variants() {
        fn assert_infallible(cs: &CascadeSource) {
            serde_json::to_value(cs)
                .expect("CascadeSource must be infallibly JSON-serializable; see rustdoc invariant");
        }
        let variants = vec![
            CascadeSource::BskyDeleteCascade { bsky_uri: "at://test/x/y".into() },
            CascadeSource::BlockCascade { block_uri: "at://test/x/y".into() },
            CascadeSource::ThreadgateCascade { post_uri: "at://test/x/y".into() },
            CascadeSource::AudienceDeleteCascade { audience_uri: "at://test/x/y".into() },
        ];
        for v in &variants {
            match v {
                CascadeSource::BskyDeleteCascade { .. }
                | CascadeSource::BlockCascade { .. }
                | CascadeSource::ThreadgateCascade { .. }
                | CascadeSource::AudienceDeleteCascade { .. } => assert_infallible(v),
            }
        }
    }

    /// SystemCleanup arm → authorized event, returns Ok.
    #[tokio::test(flavor = "multi_thread")]
    #[tracing_test::traced_test]
    async fn system_cleanup_emits_authorized() {
        let pool = fresh_shared_pool().await;
        let mut tx = pool.begin().await.expect("begin shared tx");
        let write = make_write(
            "tools.kryphocron.feed.postPrivate",
            KryphocronWriteAuthorization::SystemCleanup {
                origin: SystemCleanupOrigin::OrphanCompanionSweep { dual_link_id: 42 },
            },
        );

        bind_pipeline(&write, &mut tx, None, "did:plc:bp8", &mut Vec::new(), None)
            .await
            .expect("SystemCleanup arm must succeed");

        assert!(logs_contain("kryphocron_bind_pipeline_authorized"));
        assert!(logs_contain("SystemCleanup"));
    }
}

#[cfg(test)]
mod deny_map_step_5_tests {
    //! v0.7 arc 2 step 5 — deny-map source-1 override coverage.
    //!
    //! Asserts the three NSID-action tuples the four dedicated
    //! endpoints cover get
    //! `RequiresDedicatedEndpoint { suggested_endpoint: Some(...) }`
    //! pointing at the right XRPC procedure. Adjacent NSIDs and
    //! actions stay at the source-2 `NotYetSupported` default.

    use super::*;
    use crate::api::kryphocron_endpoints::{
        NSID_AUDIENCE, NSID_POST_PRIVATE, PROC_CREATE_POST_PRIVATE,
        PROC_DELETE_POST_PRIVATE, PROC_MANAGE_AUDIENCE,
    };

    fn assert_dedicated_endpoint(
        map: &HashMap<(String, WriteOpAction), KryphocronDenyVariant>,
        nsid: &str,
        action: WriteOpAction,
        expected_proc: &str,
    ) {
        match map.get(&(nsid.to_string(), action)) {
            Some(KryphocronDenyVariant::RequiresDedicatedEndpoint {
                suggested_endpoint: Some(proc),
            }) => {
                assert_eq!(
                    proc, expected_proc,
                    "suggested_endpoint mismatch for ({nsid}, {action:?})"
                );
            }
            other => panic!(
                "expected RequiresDedicatedEndpoint(Some({expected_proc})) for ({nsid}, {action:?}), got {other:?}"
            ),
        }
    }

    fn assert_not_yet_supported(
        map: &HashMap<(String, WriteOpAction), KryphocronDenyVariant>,
        nsid: &str,
        action: WriteOpAction,
    ) {
        match map.get(&(nsid.to_string(), action)) {
            Some(KryphocronDenyVariant::NotYetSupported) => {}
            other => panic!(
                "expected NotYetSupported for ({nsid}, {action:?}), got {other:?}"
            ),
        }
    }

    #[test]
    fn post_private_create_routes_to_create_post_private_proc() {
        let map = build_deny_map();
        assert_dedicated_endpoint(
            &map,
            NSID_POST_PRIVATE,
            WriteOpAction::Create,
            PROC_CREATE_POST_PRIVATE,
        );
    }

    #[test]
    fn post_private_delete_routes_to_delete_post_private_proc() {
        let map = build_deny_map();
        assert_dedicated_endpoint(
            &map,
            NSID_POST_PRIVATE,
            WriteOpAction::Delete,
            PROC_DELETE_POST_PRIVATE,
        );
    }

    #[test]
    fn audience_create_routes_to_manage_audience_proc() {
        let map = build_deny_map();
        assert_dedicated_endpoint(
            &map,
            NSID_AUDIENCE,
            WriteOpAction::Create,
            PROC_MANAGE_AUDIENCE,
        );
    }

    /// Update action on postPrivate has no dedicated endpoint in
    /// arc 2 — must stay at NotYetSupported.
    #[test]
    fn post_private_update_stays_not_yet_supported() {
        let map = build_deny_map();
        assert_not_yet_supported(&map, NSID_POST_PRIVATE, WriteOpAction::Update);
    }

    /// Adjacent registered NSID (`tools.kryphocron.feed.like`)
    /// stays at NotYetSupported — only the three explicit
    /// override tuples flip to RequiresDedicatedEndpoint.
    #[test]
    fn adjacent_registered_nsid_stays_not_yet_supported() {
        let map = build_deny_map();
        assert_not_yet_supported(
            &map,
            "tools.kryphocron.feed.like",
            WriteOpAction::Create,
        );
        assert_not_yet_supported(
            &map,
            "tools.kryphocron.feed.like",
            WriteOpAction::Delete,
        );
        assert_not_yet_supported(
            &map,
            "tools.kryphocron.graph.block",
            WriteOpAction::Create,
        );
    }
}
