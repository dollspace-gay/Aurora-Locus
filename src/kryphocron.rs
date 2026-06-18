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
use crate::cascade::{CascadeContext, CascadeSource, CascadeToken};
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
    // graph.block Create/Delete → dedicated endpoints (Arc H §7.2.5 / #281).
    // Generic createRecord/deleteRecord on graph.block now returns 400 with the
    // dedicated procedure suggested (mirrors postPrivate). Update stays at the
    // source-2 `NotYetSupported` default (no manageAudience-style update path).
    map.insert(
        (
            crate::api::kryphocron_endpoints::NSID_BLOCK.to_string(),
            WriteOpAction::Create,
        ),
        KryphocronDenyVariant::RequiresDedicatedEndpoint {
            suggested_endpoint: Some(
                crate::api::kryphocron_endpoints::PROC_CREATE_BLOCK.to_string(),
            ),
        },
    );
    map.insert(
        (
            crate::api::kryphocron_endpoints::NSID_BLOCK.to_string(),
            WriteOpAction::Delete,
        ),
        KryphocronDenyVariant::RequiresDedicatedEndpoint {
            suggested_endpoint: Some(
                crate::api::kryphocron_endpoints::PROC_DELETE_BLOCK.to_string(),
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
        let token = ctx.mint_token_for_test(bsky_source());

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
        let token_from_a = ctx_a.mint_token_for_test(bsky_source());

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
        let token = ctx.mint_token_for_test(bsky_source());

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
        NSID_AUDIENCE, NSID_BLOCK, NSID_POST_PRIVATE, PROC_CREATE_BLOCK,
        PROC_CREATE_POST_PRIVATE, PROC_DELETE_BLOCK, PROC_DELETE_POST_PRIVATE,
        PROC_MANAGE_AUDIENCE,
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
    }

    /// Arc H §7.2.5 / #281 — `graph.block` Create/Delete flip to
    /// `RequiresDedicatedEndpoint` (createBlock/deleteBlock); Update stays at
    /// the source-2 `NotYetSupported` default (no dedicated update path). This
    /// replaces the pre-#281 assertion that `graph.block` Create was
    /// `NotYetSupported`.
    #[test]
    fn block_endpoints_require_dedicated_endpoint() {
        let map = build_deny_map();
        assert_dedicated_endpoint(&map, NSID_BLOCK, WriteOpAction::Create, PROC_CREATE_BLOCK);
        assert_dedicated_endpoint(&map, NSID_BLOCK, WriteOpAction::Delete, PROC_DELETE_BLOCK);
        assert_not_yet_supported(&map, NSID_BLOCK, WriteOpAction::Update);
    }
}
