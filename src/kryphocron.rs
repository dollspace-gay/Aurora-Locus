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

use crate::actor_store::repository::WriteOpAction;
use crate::error::PdsError;

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
